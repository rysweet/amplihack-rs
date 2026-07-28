#!/usr/bin/env bash
# check-branch-protection.sh — detect if strict up-to-date merges got turned off.
#
# WHAT THIS CHECKS
#   The `main` branch is protected so that a pull request can only be merged
#   after it has been brought up to date with the latest `main` (GitHub calls
#   this "Require branches to be up to date before merging", stored as
#   required_status_checks.strict = true). This script reads that setting and
#   fails if it is anything other than true. If someone turns it off, this
#   check goes red so the team notices and can turn it back on.
#
# WHY THE NORMAL WORKFLOW TOKEN CANNOT BE USED
#   Reading a branch's protection settings needs admin-level read access. The
#   token GitHub Actions hands to a workflow by default (GITHUB_TOKEN) does not
#   have that access and gets a 403, even if the workflow asks for
#   administration: read. So this script must run with a dedicated
#   fine-grained Personal Access Token instead. The workflow passes that token
#   in through the GH_TOKEN environment variable (the gh CLI reads GH_TOKEN
#   automatically).
#
# HOW TO CREATE THE TOKEN SECRET (one-time setup)
#   1. GitHub -> Settings -> Developer settings -> Personal access tokens ->
#      Fine-grained tokens -> Generate new token.
#   2. Resource owner: this repository's owner. Repository access: only THIS
#      repository (not all repositories).
#   3. Permissions: Repository permissions -> Administration -> Read-only.
#      Grant nothing else.
#   4. Generate the token, copy it.
#   5. In this repository: Settings -> Secrets and variables -> Actions ->
#      New repository secret. Name it exactly BRANCH_PROTECTION_READ_TOKEN and
#      paste the token as the value.
#
# WHAT THIS IS AND IS NOT
#   This is a DETECTION control, not prevention. A repository admin can still
#   turn strict off from inside the repository; this script just makes that
#   loud by failing a check within one CI cycle (the guard runs daily on a
#   schedule, on every push to main, and on manual dispatch). The durable way
#   to PREVENT the setting from being turned off is an organization- or
#   enterprise-level ruleset, which only an org admin can set up. This script
#   is the in-repo detection layer that catches drift until/if that exists.
#
# Usage:
#   GH_TOKEN=<fine-grained PAT> scripts/check-branch-protection.sh
#
# Exit status:
#   0  required_status_checks.strict is exactly true
#   1  strict is off, the token is missing, or the API call failed
#
# Environment:
#   GH_TOKEN            required; fine-grained PAT with administration: read
#   GITHUB_REPOSITORY   owner/repo slug (set automatically in Actions); falls
#                       back to `gh repo view` when unset
#   PROTECTED_BRANCH    branch to check (defaults to main)

set -euo pipefail

BRANCH="${PROTECTED_BRANCH:-main}"

# Require the admin-read token. Absence is a loud failure, never a silent pass:
# a silent pass here would recreate the exact gap this guard exists to close.
if [ -z "${GH_TOKEN:-}" ]; then
    echo "::error::branch-protection guard not configured: missing BRANCH_PROTECTION_READ_TOKEN (fine-grained PAT, this-repo-only, administration:read)" >&2
    exit 1
fi

# Resolve the owner/repo slug: prefer the value Actions provides, else ask gh.
REPO="${GITHUB_REPOSITORY:-}"
if [ -z "$REPO" ]; then
    if ! REPO="$(gh repo view --json nameWithOwner -q .nameWithOwner)"; then
        echo "::error::could not determine repository slug (set GITHUB_REPOSITORY or run inside a gh-authenticated checkout)" >&2
        exit 1
    fi
fi

ERRFILE="$(mktemp)"
cleanup() { rm -f "$ERRFILE"; }
trap cleanup EXIT

# Read the setting with structured extraction only (gh --jq). No grep/sed/awk
# on JSON — brittle text parsing of API responses is not allowed here.
if ! strict="$(gh api "repos/${REPO}/branches/${BRANCH}/protection" --jq '.required_status_checks.strict' 2>"$ERRFILE")"; then
    echo "::error::branch-protection guard: 'gh api repos/${REPO}/branches/${BRANCH}/protection' failed (token likely lacks administration:read, or the repo/branch is wrong). gh output: $(tr '\n' ' ' <"$ERRFILE")" >&2
    exit 1
fi

if [ "$strict" = "true" ]; then
    echo "OK: branch protection on ${BRANCH}: required_status_checks.strict is 'true' (strict up-to-date merges enabled)."
    exit 0
fi

echo "::error::branch protection on ${BRANCH}: required_status_checks.strict is '${strict}', expected 'true'. Someone disabled strict up-to-date merges — re-enable immediately." >&2
exit 1
