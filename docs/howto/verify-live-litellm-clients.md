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

## Install host prerequisites

Use a Linux host with `git`, an authenticated GitHub CLI (`gh`), and
`bubblewrap` (`bwrap`) on `PATH`. The verifier uses `bwrap` to mount the
repository read-only for every client process; other operating systems fail
closed with exit `78`.

Install each selected client at the exact version in the
[live verification contract](../reference/litellm-live-verification.md).
Each binary name must resolve to exactly one distinct executable through
`PATH`. RustyClawd must be installed from the authoritative pinned git
revision, not a local path or registry package.

Install the pinned npm and RustyClawd packages, select only those client
executables on `PATH`, and confirm all prerequisites:

```bash
npm install --prefix "$HOME/.local/share/amplihack-live-clients" --save-exact \
  @anthropic-ai/claude-code@2.1.247 \
  @github/copilot@1.0.83-2
cargo install \
  --git https://github.com/rysweet/RustyClawd \
  --rev 2825862711a4bd1367022c62ed6cd2efae9f4998 \
  rustyclawd-cli

export PATH="$HOME/.local/share/amplihack-live-clients/node_modules/.bin:$HOME/.cargo/bin:/usr/bin:/bin"
command -v git gh bwrap claude copilot rusty
claude --version
copilot --version
rusty --version
gh auth status
```

## Prepare the checkout

Check out the exact open pull-request head. Commit, stash, or remove all
tracked and untracked changes, then select at least two tracked files that give
every client meaningful cross-file repository context:

```bash
git status --short
git rev-parse HEAD
gh pr view 1445 --json state,headRefOid
```

Run the command from the worktree root, or pass that absolute root with
`--repo`. The pull request must remain open and its current head must equal
`--expected-head`.

## Configure host-only inputs

Do not put credentials or endpoints on the command line:

```bash
telemetry_file="$HOME/.local/state/amplihack/litellm-telemetry/events.jsonl"
install -d -m 700 "$(dirname "$telemetry_file")"
touch "$telemetry_file"
chmod 600 "$telemetry_file"

export AMPLIHACK_LITELLM_ENDPOINT='https://gateway.example.net'
export AMPLIHACK_LITELLM_API_KEY="$(secret-tool lookup service litellm-agent)"
export AMPLIHACK_LITELLM_MODEL='gateway-coding'
export AMPLIHACK_LITELLM_EXPECTED_PROVIDER='azure'
export AMPLIHACK_LITELLM_EXPECTED_MODEL='azure/gateway-coding-deployment'
export AMPLIHACK_LITELLM_EXPECTED_GATEWAY_IDENTITY='production-coding-gateway'
export AMPLIHACK_LITELLM_TELEMETRY_FILE="$telemetry_file"
export AMPLIHACK_LITELLM_TELEMETRY_HMAC_KEY="$(secret-tool lookup service litellm-telemetry)"
```

Provider credentials remain in LiteLLM. Do not export Anthropic, OpenAI,
Azure, AWS, Google, or direct Copilot model-service credentials to the
verifier. Configure the LiteLLM success callback to append the signed schema-1
records defined by the
[live verification contract](../reference/litellm-live-verification.md).
The telemetry path must be an existing absolute regular file outside the
repository, and the HMAC key must contain at least 32 bytes. Disable gateway
caches and fallback routes for the verification alias. The signed callback's
gateway identity must exactly equal
`AMPLIHACK_LITELLM_EXPECTED_GATEWAY_IDENTITY`.

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
three. Repeat `--client` to select an explicit subset:

```bash
amplihack litellm verify-live \
  --client claude \
  --client rustyclawd \
  --pr 1445 \
  --expected-head "$head" \
  --context crates/amplihack-cli/src/commands/mod.rs \
  --context crates/amplihack-utils/src/litellm_proxy.rs
```

Do not combine `--client all` with another `--client`.

RustyClawd runs its native Copilot-compatible transport with explicit provider
and model arguments. The verifier points `GITHUB_COPILOT_ENDPOINT` at LiteLLM
and translates the virtual key into the cleared child environment. This is not
GitHub Copilot model-service fallback: telemetry must still prove the selected
LiteLLM alias and configured upstream dispatch.

The command also runs independent missing-endpoint, missing-key, missing-model,
invalid-credential, unavailable-gateway, upstream-failure, malformed-response,
cache-hit/replay, model/provider fallback, gateway-identity mismatch,
credential leakage, client-identity mismatch, and repository-modification
cases for every selected client. Any unexpected success fails the aggregate
run.

On success, stdout contains one JSON run summary with exit code `0`, client
identity and dispatch attestations, named negative-case results, totals, and
the committed evidence path and SHA-256 digest. Diagnostics go to stderr and use the
`AH-LIVE-*` identifiers listed in the
[live verification contract](../reference/litellm-live-verification.md).

## Troubleshoot a refused run

| Result | Check |
| --- | --- |
| Exit `64` | Confirm the worktree, PR head, context paths, evidence location, endpoint, model, and telemetry file meet the contract. |
| Exit `70` at `gateway-telemetry` | Confirm the callback appended one correctly signed record after the request and that its provider, model, dispatch, cache status, and result digest match. |
| Exit `77` | Remove duplicate client executables from `PATH`, install the exact release, and verify the RustyClawd Cargo receipt when selected. |
| Exit `78` | Leave CI and run on Linux with `bwrap` available. |

## Keep evidence private

Evidence is written atomically outside the repository with owner-only
permissions. Never add it to Git. It intentionally excludes prompts,
completions, source content, credentials, complete endpoints, raw gateway
records, nonce values, and environment values.
