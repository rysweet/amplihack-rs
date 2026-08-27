#!/usr/bin/env bash
# workflow_identity_preflight.sh — can the identity we are ACTING AS write to
# the repository we are about to write to? (issue #1290)
#
# The failure this exists to prevent: a run completed workflow preparation,
# workspace preparation, requirements clarification, codebase analysis,
# ambiguity resolution and host detection, and then died at
# `step-03-create-issue` with
#
#   GraphQL: Unauthorized: ... you cannot access this content (createIssue)
#
# Six steps of real work discarded over a condition that was knowable before
# the first one started. Nothing in the workflow ever asked whether the `gh`
# account that happens to be active is the account authorised for the target
# repository. `git-identity.sh` does resolve `gh api user`, but only to derive
# a COMMIT identity — it never checks that the identity can do anything.
#
# WHAT IS CHECKED, AND WHAT DELIBERATELY IS NOT
#
# The question asked here is the general one: *can the active identity write
# here*. This file does not look for "Enterprise Managed User", and it does not
# look for "Access denied by policy settings". Those are two ways of arriving at
# the general condition, and a check that recognised only them would miss an
# expired token, a stray GH_TOKEN in the environment, or an account that simply
# was never granted access. The permission answer and the generic authorisation
# vocabulary (unauthorized / forbidden / could not resolve / bad credentials)
# cover all of those at once. tests/issue_1290_identity_preflight.sh asserts
# that neither vendor-specific phrase appears in this file.
#
# WHY IT IS NOT A GATE ON ITS OWN ABILITY TO RUN
#
# #1268 is the local precedent for what a brittle gate costs: a finalisation
# gate failed a run whose work had already merged, and abandoned two live PRs
# in the process. A preflight that blocks work because it could not check is
# worse than no preflight. So the three outcomes are kept strictly apart:
#
#   the check ran and answered "no"   -> exit 1, naming account, repo, remedy
#   the check could not be made       -> WARNING on stderr, exit 0
#   there is nothing to check         -> INFO on stderr, exit 0
#     (no git repo, no origin, a non-GitHub remote, no `gh` installed)
#
# Only a positive, readable "this identity cannot act here" stops a run. Every
# unknown — no network, an API error, an unparseable answer — continues.
#
# COST
#
# Exactly one API call on the happy path. The failure path spends a second call
# on `gh auth status`, where the run is stopping anyway and naming the accounts
# the operator can switch to is worth more than the round trip.
#
# This helper is deliberately self-contained: it sources nothing. A check whose
# job is to keep a run from failing late must not acquire new ways to fail
# early.
#
# Usage:  workflow_identity_preflight.sh [REPO_PATH]
#
# Environment:
#   AMPLIHACK_SKIP_IDENTITY_PREFLIGHT     non-empty -> skip entirely (exit 0)
#   AMPLIHACK_IDENTITY_PREFLIGHT_TIMEOUT  seconds for the API call (default 30)
#
# stdout is one machine-readable line (it becomes the recipe step's context
# value). Everything human-facing goes to stderr.
#
# Exit codes:
#   0  ok / skipped / could-not-check
#   1  the active identity demonstrably cannot write to the target repository

# No `set -e`: this helper must always reach a structured answer rather than
# dying mid-probe and leaving the caller to guess what it meant.
set -uo pipefail
export GH_PAGER=cat PAGER=cat GIT_PAGER=cat LESS=FRX

REPO_ARG="${1:-${REPO_PATH:-.}}"
TIMEOUT_S="${AMPLIHACK_IDENTITY_PREFLIGHT_TIMEOUT:-30}"

note() { printf '%s\n' "$*" >&2; }

# Redact anything token-shaped before it reaches a log. Nothing in this file
# ever prints a token value, and this is the belt to that pair of braces.
redact() {
  sed -E \
    -e 's#gh[pousr]_[A-Za-z0-9_]{8,}#<redacted-token>#g' \
    -e 's#github_pat_[A-Za-z0-9_]{8,}#<redacted-token>#g' \
    -e 's#https?://[^[:space:]/]*@#https://<redacted>@#g' \
    -e 's#[Bb]earer[[:space:]]+[A-Za-z0-9._~+/=-]{20,}#Bearer <redacted-token>#g'
}

# skip <reason> — nothing to check here.
skip() {
  note "INFO: identity preflight skipped: $1"
  printf 'identity_preflight: skipped reason=%s\n' "$1"
  exit 0
}

# unknown <reason> — the check itself could not be made. Warn, do not gate.
unknown() {
  note "WARNING: identity preflight could not be completed ($1) — continuing without an identity check. This is advisory: an un-runnable check must not stop work (issue #1268)."
  printf 'identity_preflight: unknown reason=%s\n' "$1"
  exit 0
}

# ---------------------------------------------------------------------------
# Failure-path helpers: name the account, and the accounts one could switch to.
# ---------------------------------------------------------------------------

auth_status_text() {
  local host="$1"
  if command -v timeout >/dev/null 2>&1; then
    timeout "$TIMEOUT_S" gh auth status --hostname "$host" 2>&1 | redact
  else
    gh auth status --hostname "$host" 2>&1 | redact
  fi
}

# The account gh reports as active for this host, or empty.
active_account_from_auth_status() {
  printf '%s\n' "$1" | awk '
    { for (i = 1; i < NF; i++) if ($i == "account") { acct = $(i + 1); gsub(/[^A-Za-z0-9._-]/, "", acct) } }
    /Active account: true/ { if (acct != "") { print acct; exit } }
  '
}

# Every account gh knows about for this host, space separated.
known_accounts_from_auth_status() {
  printf '%s\n' "$1" \
    | grep -Eo 'account[[:space:]]+[A-Za-z0-9._-]+' \
    | awk '{ print $2 }' | sort -u | tr '\n' ' ' | sed 's/ *$//'
}

# deny <login> <host> <owner/name> <origin-url> <evidence>
#
# Every one of the three facts the issue asks for is mandatory here: which
# account is active, which repository was targeted, and how to switch.
deny() {
  local login="$1" host="$2" repo="$3" origin="$4" evidence="$5"
  local status_text known
  status_text="$(auth_status_text "$host")"
  [ -n "$login" ] || login="$(active_account_from_auth_status "$status_text")"
  [ -n "$login" ] || login="<could not be resolved>"
  known="$(known_accounts_from_auth_status "$status_text")"
  [ -n "$known" ] || known="<none reported by gh auth status>"

  {
    echo "ERROR: identity preflight failed — the active GitHub account cannot write to the target repository."
    echo ""
    echo "  active account    : ${login}"
    echo "  host              : ${host}"
    echo "  target repository : ${repo}"
    echo "  origin remote     : ${origin}"
    echo "  evidence          : ${evidence}"
    echo ""
    echo "  This is checked once, before any step does real work. The same condition"
    echo "  otherwise surfaces several steps later, after requirements, analysis and"
    echo "  design have already run, and throws that work away (issue #1290)."
    echo ""
    echo "  To act as a different account:"
    echo "    gh auth status                                  # which accounts gh knows"
    echo "    gh auth switch --hostname ${host} --user <ACCOUNT>"
    echo "    gh auth login --hostname ${host}                # if it is not added yet"
    echo "  Accounts gh currently knows on ${host}: ${known}"
    if [ -n "${GH_TOKEN:-}" ] || [ -n "${GITHUB_TOKEN:-}" ]; then
      echo ""
      echo "  NOTE: GH_TOKEN or GITHUB_TOKEN is set in this environment (value not"
      echo "  shown). An exported token supplies the identity directly and overrides"
      echo "  \`gh auth switch\` entirely — unset it first, or the switch changes nothing."
    fi
    echo ""
    echo "  If this check is wrong for your setup — for example you intend to push to a"
    echo "  fork rather than to origin — set AMPLIHACK_SKIP_IDENTITY_PREFLIGHT=1."
  } >&2

  printf 'identity_preflight: denied login=%s repo=%s\n' "$login" "$repo"
  exit 1
}

# ---------------------------------------------------------------------------
# 1. Is there anything to check?
# ---------------------------------------------------------------------------

[ -n "${AMPLIHACK_SKIP_IDENTITY_PREFLIGHT:-}" ] && skip "AMPLIHACK_SKIP_IDENTITY_PREFLIGHT is set"

cd "$REPO_ARG" 2>/dev/null || unknown "repo-path-unreadable"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || skip "not-a-git-repository"

ORIGIN="$(git remote get-url origin 2>/dev/null)" || ORIGIN=""
[ -n "$ORIGIN" ] || skip "no-origin-remote"
ORIGIN_SAFE="$(printf '%s' "$ORIGIN" | redact)"

# Host and owner/name from the origin URL. Both URL shapes git uses:
#   scheme://[user@]host[:port]/owner/repo[.git]
#   [user@]host:owner/repo[.git]                      (scp-like)
if [ "${ORIGIN#*://}" != "$ORIGIN" ]; then
  REST="${ORIGIN#*://}"; REST="${REST##*@}"
  HOST="${REST%%/*}"; HOST="${HOST%%:*}"
  RPATH="${REST#*/}"
else
  REST="${ORIGIN##*@}"
  HOST="${REST%%:*}"
  RPATH="${REST#*:}"
fi
HOST="$(printf '%s' "$HOST" | tr '[:upper:]' '[:lower:]')"
RPATH="${RPATH%.git}"; RPATH="${RPATH%/}"
OWNER="${RPATH%%/*}"
NAME="${RPATH#*/}"; NAME="${NAME%%/*}"

case "$OWNER" in ''|*[!A-Za-z0-9._-]*) skip "origin-not-parseable-as-owner/repo" ;; esac
case "$NAME" in ''|*[!A-Za-z0-9._-]*) skip "origin-not-parseable-as-owner/repo" ;; esac

# Only GitHub remotes are in scope. Azure DevOps and everything else have their
# own credential paths and are not this check's business.
case "$HOST" in
  github.com|*.ghe.com|*.githubenterprise.com) ;;
  *) [ "$HOST" = "${GH_HOST:-}" ] || skip "non-github-remote:${HOST}" ;;
esac

command -v gh >/dev/null 2>&1 || unknown "gh-not-installed"

REPO_SLUG="${OWNER}/${NAME}"

# ---------------------------------------------------------------------------
# 2. The single API call: who are we, and what may we do here?
# ---------------------------------------------------------------------------
#
# One round trip answers both halves. `viewerPermission` is the repository
# permission of the authenticated viewer — READ, TRIAGE, WRITE, MAINTAIN,
# ADMIN — which is exactly "can this identity write here", asked directly
# instead of inferred from a refusal after the fact.

# shellcheck disable=SC2016  # GraphQL variables, not shell expansions.
QUERY='query($owner:String!,$name:String!){viewer{login} repository(owner:$owner,name:$name){nameWithOwner viewerPermission}}'

ERRFILE="$(mktemp "${TMPDIR:-/tmp}/identity-preflight.XXXXXX")" || unknown "cannot-create-tempfile"
# shellcheck disable=SC2329  # invoked by the EXIT trap below.
cleanup() { rm -f "$ERRFILE"; }
trap cleanup EXIT

RC=0
if command -v timeout >/dev/null 2>&1; then
  RESP="$(timeout "$TIMEOUT_S" gh api graphql --hostname "$HOST" \
    -f owner="$OWNER" -f name="$NAME" -f query="$QUERY" 2>"$ERRFILE")" || RC=$?
else
  RESP="$(gh api graphql --hostname "$HOST" \
    -f owner="$OWNER" -f name="$NAME" -f query="$QUERY" 2>"$ERRFILE")" || RC=$?
fi

# The response is small and flat; each key appears once. Extracting with sed
# keeps this helper free of a `jq` dependency, which is one less reason for the
# check to be un-runnable on a machine where the run itself would be fine.
json_value() {
  printf '%s' "$2" | tr ',{}' '\n' \
    | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -n1
}

LOGIN="$(json_value login "$RESP")"

if [ "$RC" -eq 0 ]; then
  PERMISSION="$(json_value viewerPermission "$RESP")"
  case "$PERMISSION" in
    ADMIN|MAINTAIN|WRITE)
      note "INFO: identity preflight: acting as '${LOGIN:-<unknown>}' on ${HOST}; ${REPO_SLUG} permission=${PERMISSION}."
      printf 'identity_preflight: ok login=%s repo=%s permission=%s\n' "${LOGIN:-unknown}" "$REPO_SLUG" "$PERMISSION"
      exit 0
      ;;
    '')
      # A 200 that carries no permission is an answer we cannot read, not a no.
      unknown "permission-not-reported-by-api"
      ;;
    *)
      deny "$LOGIN" "$HOST" "$REPO_SLUG" "$ORIGIN_SAFE" \
        "the API reports viewerPermission=${PERMISSION} for this account, which cannot push branches or open issues here"
      ;;
  esac
fi

# ---------------------------------------------------------------------------
# 3. The call failed. Separate "the identity was refused" from "we could not
#    ask". Anything that is not a readable refusal continues the run.
# ---------------------------------------------------------------------------

ERRTEXT="$(head -c 2000 "$ERRFILE" 2>/dev/null | redact | tr '\n' ' ')"
[ -n "$ERRTEXT" ] || ERRTEXT="(no error output; gh exited ${RC})"

# `timeout` kills with 124; that is never a verdict about an identity.
if [ "$RC" -eq 124 ] || [ "$RC" -eq 137 ]; then
  unknown "api-call-timed-out-after-${TIMEOUT_S}s"
fi

# Rate limiting is checked FIRST because GitHub answers both "slow down" and
# "you may not" with 403, and only the first of those must not stop a run.
if printf '%s' "$ERRTEXT" | grep -Eiq 'rate limit|secondary rate|abuse detection|retry-after|x-ratelimit'; then
  unknown "rate-limited"
fi

# Transport and server-side trouble: no verdict is available.
if printf '%s' "$ERRTEXT" | grep -Eiq 'no such host|dial tcp|connection (refused|reset)|timed? ?out|tls handshake|network is unreachable|temporary failure|EOF|HTTP 5[0-9][0-9]|(^|[^0-9])(500|502|503|504)([^0-9]|$)|proxy'; then
  unknown "network-or-server-error"
fi

# A readable refusal of THIS identity, in the generic vocabulary GitHub uses
# for it. Note what is absent: no product name, no policy phrase, no EMU.
if printf '%s' "$ERRTEXT" | grep -Eiq 'could not resolve to a repository|resource not accessible|not authoriz|unauthoriz|forbidden|denied|permission|bad credentials|requires authentication|not logged in|must be a member|single sign-?on|saml|(^|[^0-9])(401|403|404)([^0-9]|$)|cannot access'; then
  deny "$LOGIN" "$HOST" "$REPO_SLUG" "$ORIGIN_SAFE" "$ERRTEXT"
fi

unknown "unrecognised-api-error: ${ERRTEXT}"
