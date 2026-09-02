---
title: LiteLLM Live Verification Contract
description: Security and evidence contract for the host-only real-client LiteLLM verifier.
last_updated: 2026-09-02
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: reference
---

# LiteLLM Live Verification Contract

`amplihack litellm verify-live` is a host-only, fail-closed verification rail.
It attests an exact pull-request head and the real Claude Code, GitHub Copilot
CLI, and RustyClawd executables before any live inference credential can be
read. It is separate from the ordinary client launch paths.

## Command

```text
amplihack litellm verify-live \
  [--client all|claude|copilot|rustyclawd] \
  --pr NUMBER \
  --expected-head 40_HEX_SHA \
  --context TRACKED_RELATIVE_FILE \
  --context TRACKED_RELATIVE_FILE \
  [--repo PATH] \
  [--evidence-dir ABSOLUTE_PATH] \
  [--timeout-seconds 10..900]
```

The repository must be clean and exactly at both the supplied SHA and the open
pull request's current head. Supply 2-24 distinct tracked regular files.
Evidence must be outside the worktree.

## Attested clients

| Client | Binary | Exact version | Protocol |
| --- | --- | --- | --- |
| Claude Code | `claude` | `2.1.247` | Anthropic messages |
| GitHub Copilot CLI | `copilot` | `1.0.83-2` | OpenAI chat completions |
| RustyClawd | `rusty` | `0.1.1` | OpenAI chat completions |

RustyClawd additionally requires an unambiguous Cargo git receipt for
`https://github.com/rysweet/RustyClawd` at revision
`2825862711a4bd1367022c62ed6cd2efae9f4998`. Registry, path, branch-only,
tag-only, and alternate-binary receipts are rejected.

The verifier uses RustyClawd's native Copilot-compatible backend with an
explicit `--provider copilot`, exact model alias, and
`GITHUB_COPILOT_ENDPOINT=<gateway>/v1`. It translates the LiteLLM virtual key
to `GITHUB_TOKEN` only inside RustyClawd's cleared child environment. The
explicit provider prevents RustyClawd's implicit Anthropic-to-Copilot fallback.
No wrapper, traffic interception, substitute binary, or direct provider route
is used.

## Live configuration

The verifier requires the ordinary three route variables plus a signed,
append-only gateway telemetry source:

```text
AMPLIHACK_LITELLM_ENDPOINT
AMPLIHACK_LITELLM_API_KEY
AMPLIHACK_LITELLM_MODEL
AMPLIHACK_LITELLM_EXPECTED_PROVIDER
AMPLIHACK_LITELLM_EXPECTED_MODEL
AMPLIHACK_LITELLM_TELEMETRY_FILE
AMPLIHACK_LITELLM_TELEMETRY_HMAC_KEY
```

The expected provider and model are the exact values emitted by LiteLLM's
standard logging payload for the one approved deployment behind the alias.
They are mandatory so a signed record from a fallback deployment cannot pass.

The telemetry file is external to the repository. Each JSONL record has schema
version `1` and fields `correlation_id`, `requested_alias`,
`observed_provider`, `observed_model`, `gateway_identity`, `cache_status`,
`backend_dispatch_id`, `result_sha256`, and `signature_sha256`. The signature
is lowercase HMAC-SHA-256 over the preceding values in that order, separated
by newline characters. The HMAC key must contain at least 32 bytes.

LiteLLM's operator-owned success callback must append exactly one record for
each correlation ID. A record passes only when it authenticates, names the
requested alias and actual upstream provider/model, identifies a backend
dispatch, and reports `cache_status=miss`. A missing, duplicate, replayed,
unsigned, cache-hit, or model-mismatched record fails closed.

## Exit codes

| Code | Meaning |
| ---: | --- |
| 0 | All positive proofs and negative cases passed |
| 1 | A completed deterministic expectation failed |
| 2 | Command grammar or value error |
| 64 | Missing, partial, conflicting, or unsafe configuration |
| 69 | Positive live gateway unavailable |
| 70 | Typed protocol, telemetry, or evidence failure |
| 77 | Client identity, provenance, integrity, or capability failure |
| 78 | CI refusal or missing host isolation primitive |
| 130 | User interruption |

## CI boundary

CI markers are checked from raw arguments before update checks, self-heal,
Clap value validation, endpoint or credential access, subprocess launch, or
evidence creation. Live verification prints diagnostic `AH-LIVE-CI-001` and
returns `78`. Help and parse errors remain available without ordinary startup
side effects. CI coverage uses only deterministic Rust tests and loopback
fixtures; workflows must never install or invoke real clients or provider-backed
inference for this command.

## Evidence

Passing evidence is a schema-versioned JSONL record in an owner-only directory
and file. The default is
`$XDG_STATE_HOME/amplihack/litellm-evidence`, or
`$HOME/.local/state/amplihack/litellm-evidence` when `XDG_STATE_HOME` is
unset. It contains client and binary identity, repository and context digests,
correlation and dispatch identifiers, requested and observed models,
cache status, result digests, negative-case totals, and RustyClawd provenance.
It excludes credentials, token hashes, endpoint values, prompts, repository
content, raw client output, response bodies, environment values, and nonces.
