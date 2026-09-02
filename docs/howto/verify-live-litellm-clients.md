---
title: Verify Real LiteLLM Clients on a Trusted Host
description: Run the fail-closed real-client LiteLLM verifier against an exact pull-request head.
last_updated: 2026-09-02
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: howto
---

# Verify Real LiteLLM Clients on a Trusted Host

Run live verification only on a trusted local host. CI always refuses it with
exit `78`.

## Prepare the checkout

Check out the exact open pull-request head, remove untracked files, and select
at least two tracked files that give every client meaningful cross-file
repository context:

```bash
git status --short
git rev-parse HEAD
gh pr view 1445 --json state,headRefOid
```

Install the exact client releases listed in the
[live verification contract](../reference/litellm-live-verification.md).
RustyClawd must be installed from the authoritative pinned git revision, not a
local path or registry package.

## Configure host-only inputs

Do not put credentials or endpoints on the command line:

```bash
export AMPLIHACK_LITELLM_ENDPOINT='https://gateway.example.net'
export AMPLIHACK_LITELLM_API_KEY="$(secret-tool lookup service litellm-agent)"
export AMPLIHACK_LITELLM_MODEL='gateway-coding'
export AMPLIHACK_LITELLM_EXPECTED_PROVIDER='azure'
export AMPLIHACK_LITELLM_EXPECTED_MODEL='azure/gateway-coding-deployment'
export AMPLIHACK_LITELLM_TELEMETRY_FILE="$HOME/.local/state/litellm/events.jsonl"
export AMPLIHACK_LITELLM_TELEMETRY_HMAC_KEY="$(secret-tool lookup service litellm-telemetry)"
```

Provider credentials remain in LiteLLM. Do not export Anthropic, OpenAI,
Azure, AWS, Google, or direct Copilot model-service credentials to the
verifier. Configure the LiteLLM success callback to append the signed schema-1
records defined by the
[live verification contract](../reference/litellm-live-verification.md).
Disable gateway caches and fallback routes for the verification alias.

## Run the verifier

```bash
head="$(git rev-parse HEAD)"
amplihack litellm verify-live \
  --pr 1445 \
  --expected-head "$head" \
  --context crates/amplihack-cli/src/commands/mod.rs \
  --context crates/amplihack-utils/src/litellm_proxy.rs \
  --evidence-dir "$HOME/.local/state/amplihack/litellm-evidence"
```

The verifier never treats another client's success as a substitute. Each
selected client independently returns fresh, substantive cross-file analysis
through the requested alias, and signed gateway telemetry proves a cache-miss
backend dispatch with no fallback. The default `--client all` verifies all
three. Repeat `--client` to select an explicit subset.

RustyClawd runs its native Copilot-compatible transport with explicit provider
and model arguments. The verifier points `GITHUB_COPILOT_ENDPOINT` at LiteLLM
and translates the virtual key into the cleared child environment. This is not
GitHub Copilot model-service fallback: telemetry must still prove the selected
LiteLLM alias and configured upstream dispatch.

The command also runs independent missing-endpoint, missing-key, missing-model,
invalid-credential, unavailable-gateway, upstream-failure, and
malformed-response cases for every selected client. Any unexpected success
fails the aggregate run.

## Keep evidence private

Evidence is written atomically outside the repository with owner-only
permissions. Never add it to Git. It intentionally excludes prompts,
completions, source content, credentials, complete endpoints, raw gateway
records, nonce values, and environment values.
