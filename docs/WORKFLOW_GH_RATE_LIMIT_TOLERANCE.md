# GitHub API Rate-Limit Tolerance for Default-Workflow Tooling

The default-workflow helper scripts that talk to GitHub (`gh`) are tolerant of
GitHub API rate limits. A temporarily-exhausted API quota no longer fails a
workflow closed: rate-limited calls wait for the **authoritative reset** or fall
back to a REST read on a different budget, while permanent errors (bad
credentials) fail fast and generic transient errors keep their short retry.

- **Audience:** contributors working on `amplifier-bundle/tools/*` workflow
  helpers, and operators debugging a workflow that stalled or failed on a
  GitHub API error.
- **Scope:** the shared retry library `workflow_gh_retry.sh` and the four helper
  scripts that source it. No behavior outside the default-workflow tooling is
  affected.

## Contents

- [Why this exists](#why-this-exists)
- [Quick start](#quick-start)
- [Error classification](#error-classification)
- [The shared library: `workflow_gh_retry.sh`](#the-shared-library-workflow_gh_retrysh)
  - [Configuration (environment variables)](#configuration-environment-variables)
  - [Public functions](#public-functions)
- [How each helper uses it](#how-each-helper-uses-it)
- [Examples](#examples)
- [Security: credential redaction](#security-credential-redaction)
- [Testing](#testing)
- [Design notes](#design-notes)

## Why this exists

When the GitHub GraphQL quota is exhausted (`0/5000`, reset up to ~1h later),
the previous helpers classified "rate limit" as a generic transient error and
retried roughly three times with an immediate/short backoff. Every retry hit the
still-exhausted quota and the step failed closed — aborting an otherwise healthy
workflow run.

The rate-limit-aware helpers distinguish three error classes and handle each
appropriately, so a quota blip becomes a bounded wait instead of a failure.

## Quick start

The tooling is used automatically by the default-workflow recipes — there is
nothing to enable. The scripts source the shared library and route their `gh`
calls through it:

```bash
# Inside a helper script (e.g. workflow_pr_ready.sh)
GH_RETRY_HELPER="${WORKFLOW_GH_RETRY_HELPER:-${SCRIPT_DIR}/workflow_gh_retry.sh}"
# shellcheck source=/dev/null
. "$GH_RETRY_HELPER"

# A read that may fall back to REST on a rate limit:
GH_RETRY_REST_FALLBACK=_scope_rest_view_url \
  _gh_retry_core "pr view" pr view "$PR_URL" --repo "$REPO" --json "$fields"

# A mutation that must wait-for-reset (no REST fallback):
_gh_retry_core "pr ready" pr ready "$PR_NUMBER" --repo "$REPO"
```

Sourcing the library has **no side effects** — it only defines functions.

## Error classification

`classify_gh_error <stderr_file>` reads a command's captured stderr and echoes
exactly one of four classes. Order matters: permanent auth errors are checked
first (never retried), then rate-limit signals, then generic transient signals.

| Class        | Example signals                                                                 | Handling |
| ------------ | ------------------------------------------------------------------------------- | -------- |
| `auth`       | `Bad credentials`, `HTTP 401`, `requires authentication`, `not logged in`, expired/invalid token | **Never retried.** Fail fast with a redacted message. |
| `rate_limit` | `API rate limit exceeded`, `secondary rate limit`, `abuse detection`, `Retry-After`, `X-RateLimit-Reset`, `HTTP 429`, and a bare `HTTP 403` that is *not* a permission error | Wait for the **authoritative reset** and/or serve via a REST fallback. |
| `transient`  | `HTTP 5xx`, `timed out`, `connection reset/refused`, `TLS handshake`, `network`, `EOF` | Short exponential backoff, bounded attempts (default 3). |
| `other`      | Anything else, including a `403` that clearly reads as a permission error (`resource not accessible`, `must have admin`, `insufficient`, `permission`) | Not retried; returns the original exit status. |

> A bare `403` is treated as a (secondary) rate-limit signal **unless** the
> message clearly indicates a permission problem, in which case it is `other`
> (non-retryable).

## The shared library: `workflow_gh_retry.sh`

Location: `amplifier-bundle/tools/workflow_gh_retry.sh`. It is sourced by the
four default-workflow helper scripts. It defines a classifier, authoritative
reset lookups, an adaptive wait, REST fallbacks shaped like `gh --json` output,
and the retry driver `_gh_retry_core`.

### Configuration (environment variables)

All variables are optional. The first five are read by `_gh_retry_core` to
tune retry behavior; `WORKFLOW_GH_RETRY_HELPER` is read by the sourcing helper
scripts to locate the library before it is sourced.

| Variable                 | Type / default        | Effect |
| ------------------------ | --------------------- | ------ |
| `GH_RETRY_RESOURCE`      | `graphql` \| `core` (default `graphql`) | Which rate-limit resource to consult for the reset epoch on a rate-limit. |
| `GH_RETRY_MAX_TRANSIENT` | integer (default `3`) | Attempts for the generic transient path (preserves legacy behavior). |
| `GH_RETRY_MAX_RL_WINDOWS`| integer (default unset) | Optional safety valve capping how many reset windows to wait. **Unset = no arbitrary cap** — keep honoring the authoritative reset. Set to `0` to fail fast on a rate limit (used by display-only reads). |
| `GH_RETRY_REST_FALLBACK` | function name (default unset) | READ paths only: on a rate limit, serve the request via this REST fallback (core budget) instead of blocking, logging a `WARNING`. Leave **unset** for write paths so they wait-for-reset. |
| `GH_RETRY_TMPDIR`        | directory (default `.`) | Where the per-attempt stderr capture files are written. |
| `WORKFLOW_GH_RETRY_HELPER` | path (default `${SCRIPT_DIR}/workflow_gh_retry.sh`) | Read by the sourcing helper scripts (not `_gh_retry_core`): lets a caller/test point at an alternate library location. |

After a call, `_gh_retry_core` sets the cross-script global
**`GH_RETRY_LAST_CLASS`** to `success`, `rate_limit_rest`, or the class of the
final failure (`auth`, `rate_limit`, `transient`, `other`), so callers can
branch on how a result was obtained.

### Public functions

| Function | Purpose | Returns |
| -------- | ------- | ------- |
| `classify_gh_error <stderr_file>` | Classify captured stderr. | echoes `auth`\|`rate_limit`\|`transient`\|`other` |
| `_gh_retry_core <label> <gh-arg>...` | The retry driver. Runs `gh <args>` with a 60s per-attempt `timeout`, classifying failures and acting per class. | stdout = command (or REST fallback) output; return = `0` on success else last `gh` exit status. Sets `GH_RETRY_LAST_CLASS`. |
| `gh_rate_limit_reset_epoch <resource>` | Authoritative reset epoch via `gh api rate_limit` (does **not** consume the core/graphql budget). | echoes epoch seconds, or returns `1` |
| `gh_rate_limit_remaining <resource>` | Remaining request count for a resource. | echoes integer, or returns `1` |
| `gh_reset_epoch_from_stderr <stderr_file>` | Fallback reset lookup that honors a `Retry-After: <seconds>` or `X-RateLimit-Reset: <epoch>` header when `gh api rate_limit` is unavailable. | echoes epoch seconds, or returns `1` |
| `gh_wait_for_rate_limit <resource> <stderr_file>` | Adaptive sleep until just after the authoritative reset (floor `5s`, buffer `3s`, **no fixed cap**). | sleeps; always returns after waking |
| `gh_pr_exists_rest <OWNER/REPO> <branch>` | REST `/pulls` existence check, emitting a JSON **array** shaped like `gh pr list --json ...`. | echoes JSON array, or returns `1` |
| `gh_pr_view_rest <OWNER/REPO> <pr_number>` | REST `/pulls/:n` read, emitting a single JSON **object** shaped like `gh pr view --json ...`. | echoes JSON object, or returns `1` |
| `gh_pr_number_from_url <url>` | Extract the trailing PR number from a PR URL. | echoes integer, or returns `1` |
| `gh_sanitize_stderr <stderr_file>` | Redact embedded credentials/tokens from a stderr capture (first 500 chars, single line). | echoes redacted text |

The REST fallbacks (`gh_pr_exists_rest`, `gh_pr_view_rest`) normalize REST JSON
into the same field set the workflow already consumes from `gh --json`:
`number, title, body, state (OPEN/MERGED/CLOSED), createdAt, mergedAt, url,
headRefName, baseRefName, headRefOid, headRepositoryOwner.login,
headRepository.name, isCrossRepository`. Callers consume REST and GraphQL output
identically.

## How each helper uses it

| Helper script | Role | Rate-limit policy |
| ------------- | ---- | ----------------- |
| `workflow_publish_pr.sh` | Create/find the PR for the branch. | Reads (`pr list`, `pr view`) set `GH_RETRY_REST_FALLBACK` to REST helpers so a GraphQL outage still resolves PR existence. `pr create` is a write: wait-for-reset, no REST fallback. |
| `workflow_pr_scope.sh` | Validate a PR's scope (head/base refs, oid, cross-repo, title prefix, created-after). | Reads use REST fallbacks (`_scope_rest_view_url`, `_scope_rest_view_number`, `_scope_rest_list`) via a small `gh_with_retry` wrapper. |
| `workflow_pr_ready.sh` | Mark PR ready and comment — **mutations**. | No REST fallback configured: a genuine rate limit **waits for the authoritative reset** and retries. Auth errors never retried. |
| `workflow_final_status.sh` | Display-only final PR status read. | Pins `GH_RETRY_MAX_RL_WINDOWS=0` so a rate limit **fails fast** instead of sleeping through a reset window; transient errors still get the short bounded retries. No REST fallback. |

The `workflow-prep.yaml` recipe also creates the tracking issue with the same
classifier / wait-for-reset policy, kept **inline** on purpose: that embedded
step runs before any tool script is sourced. Keep the inline logic and
`workflow_gh_retry.sh` in sync when either changes.

## Examples

### A read that prefers REST on a rate limit

```bash
_publish_rest_list() { gh_pr_exists_rest "$REPO" "$HEAD_REF"; }

GH_RETRY_REST_FALLBACK=_publish_rest_list \
  _gh_retry_core "pr list" pr list --repo "$REPO" --head "$HEAD_REF" \
    --state all --json number,state,url,headRefOid

case "$GH_RETRY_LAST_CLASS" in
  success)         echo "served via GraphQL" ;;
  rate_limit_rest) echo "GraphQL rate-limited; served via REST core budget" ;;
esac
```

On a GraphQL rate limit, if the core budget still has requests the driver logs a
`WARNING` and returns the REST result immediately (`GH_RETRY_LAST_CLASS=rate_limit_rest`)
rather than blocking for the GraphQL reset.

### A mutation that must wait for reset

```bash
# No GH_RETRY_REST_FALLBACK set -> write path.
_gh_retry_core "pr ready" pr ready "$PR_NUMBER" --repo "$REPO"
# On a rate limit this waits until just after the authoritative reset
# (adaptive, no fixed cap), then retries. Auth errors return immediately.
```

### A display read that must not stall

```bash
# Fail fast on a rate limit; never sleep through a (possibly hour-long) reset.
GH_RETRY_MAX_RL_WINDOWS=0 \
  _gh_retry_core "final PR status pr view" pr view "$PR_NUMBER" \
    --repo "$REPO" --json state,mergedAt,url || true
```

### Consulting the authoritative reset directly

```bash
# These use `gh api rate_limit`, which does NOT consume the core/graphql budget.
reset_epoch="$(gh_rate_limit_reset_epoch graphql)"   # epoch seconds
core_left="$(gh_rate_limit_remaining core)"          # remaining requests
```

## Security: credential redaction

Every logged error is passed through `gh_sanitize_stderr`, which redacts:

- Basic-auth credentials embedded in URLs: `https://user:token@host` → `https://REDACTED@host`
- GitHub tokens: `ghp_/gho_/ghu_/ghs_/ghr_...` → `REDACTED_TOKEN`
- Fine-grained PATs: `github_pat_...` → `REDACTED_TOKEN`

Output is collapsed to a single line and truncated to 500 characters so a raw
token can never reach the workflow log.

## Testing

Two test suites cover the behavior. Run them from the repo root.

Shell contract/backoff tests:

```bash
bash amplifier-bundle/recipes/tests/test-gh-rate-limit-backoff.sh
bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
```

These stub `gh` to emit rate-limit, auth, transient, and success stderr and
assert the classifier, adaptive wait (mocked `sleep`), REST-fallback routing,
`GH_RETRY_LAST_CLASS` values, and token redaction.

Rust integration resilience tests. These are `[[test]]` targets of the
`amplihack` package (defined in `bins/amplihack/Cargo.toml`, sources under
`tests/integration/`), so scope them with `-p amplihack`:

```bash
cargo test -p amplihack --test workflow_publish_resilience
cargo test -p amplihack --test workflow_finalize_resilience
```

## Design notes

- **No silent fallbacks** (see `amplifier-bundle/context/PHILOSOPHY.md`). Every
  fallback is logged at `WARNING`, and the caller still fails closed when both
  GraphQL and REST are exhausted.
- **No arbitrary time cap.** The wait is bounded by the *observed reset window*,
  not a fixed number — a `floor` (5s) and `buffer` (3s) guard against negative,
  absent, or clock-skewed values. This is a liveness-bounded sleep, not a wall
  clock, consistent with the workflow NO-TIMEOUT policy.
- **Reset lookups are free.** `gh api rate_limit` does not consume the
  core/graphql budget, so probing the reset never deepens the exhaustion.
- **Read vs. write asymmetry.** Reads may serve from REST on the core budget to
  stay responsive; writes must wait for the authoritative reset so no mutation
  is silently skipped or duplicated.
