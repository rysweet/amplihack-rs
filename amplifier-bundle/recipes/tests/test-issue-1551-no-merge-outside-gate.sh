#!/usr/bin/env bash
# test-issue-1551-no-merge-outside-gate.sh — issue #1551: a merge attempted
# anywhere but autodrive_merge_gate.sh is refused, in every auto-drive phase.
#
# The `gh` shim at amplifier-bundle/tools/gh-merge-guard/gh sits first on PATH
# in front of a stub "real" gh that logs its argv and keeps the PR OPEN unless
# a merge reaches it. PATH is the guard directory, the stub directory and a
# directory of links to the system tools with every `gh` left out, so no real
# gh can be reached.
#
#   T1  every merge form, run from a phase-1 agent's shell, exits 3 with
#       REFUSED; the stub sees no merge and the PR stays OPEN
#   T2  forged gates are refused: `bash -c` text naming the gate, a file named
#       like the gate, a byte-identical copy, `exec -a`, a child of the gate
#   T3  the trusted gate's own merge, run as `<root bash> -p <physical gate>`,
#       reaches the real gh, directly and from `$(…)` and a pipeline; the same
#       gate without -p, through a symlink or by a relative path is refused
#       (G6); the real gate's NOT_MERGED path still exits 1 (the real gate's
#       MERGED path, with the full evidence harness, is section 5i-1551 of
#       test-auto-drive-to-merge.sh)
#   T4  non-merge calls pass through with the same argv, stdin, stdout, stderr
#       and exit code; a refusal never echoes argv
#   T5  no real gh, two shim copies, or a wrapper that re-enters: exit 127
#   SEC security regressions: attached and unknown flags, dot segments and
#       odd bytes in endpoints, a hostile relative ./gh, arithmetic injection
#       through the depth variable, exported functions and BASH_ENV, alias
#       names with pattern characters, and secrets kept out of the refusal;
#       G6 a gate whose environ carries BASH_FUNC_*, BASH_ENV, ENV,
#       SHELLOPTS, BASHOPTS, PS4 or an LD_* loader variable is refused, and
#       step-03's launch clears them so a legitimate session still merges; G9 the refusal tells the agent
#       not to retry; G12 the shim re-checks its parent pid; IV-2 URL,
#       prefix, query, placeholder and mixed-case REST forms are refused
#   T6  the advisory directive: autodrive-build step-01b (G1: fails closed
#       when jq fails or the directive is missing) and the prompt lines (G3:
#       merge-round adds only the directive line)
#   D   detection (G4, G7, G8): a PR found MERGED after phase 1 or phase 2
#       ends the run with exit 1; an unreadable state is read twice, then exit
#       1; the state helper comes from AMPLIHACK_HOME only, never REPO_PATH
#   S   static: mode, shebang, marker, no /tmp, no eval, CI wiring; G6 gate
#       shebang `#!/bin/bash -p`, `unset -f`, step-03's `/bin/bash -p`
#
# Linux only: the gate check reads /proc. Uses $TMPDIR for every file.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1551-no-merge-outside-gate.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
GUARD_DIR="${TOOLS}/gh-merge-guard"
SHIM="${GUARD_DIR}/gh"
REAL_GATE="${TOOLS}/autodrive_merge_gate.sh"
CI_YML="${REPO_ROOT}/.github/workflows/ci.yml"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }
harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

DIRECTIVE='[amplihack:no-merge] Do NOT run "gh pr merge" or call the pulls/<N>/merge API, and do not list merging in requirements or acceptance criteria. Leave the PR open. Auto-drive'"'"'s merge gate merges it after a Crusty CLEAN and the merge-ready loop; any other merge attempt is refused.'
MARKER='[amplihack:no-merge]'
REFUSED_PREFIX='REFUSED (amplihack #1551): '
REFUSED_SUFFIX=' is only allowed inside autodrive_merge_gate.sh'

[ -d /proc/self ] || harness_error "this test needs Linux /proc"
[ -f "${REAL_GATE}" ] || harness_error "missing ${REAL_GATE}"
command -v jq >/dev/null 2>&1 || harness_error "jq is required"

# A root-owned bash: the shim only trusts a gate run by one (check 3).
BASH_BIN=""
for cand in "$(command -v bash 2>/dev/null || true)" /usr/bin/bash /bin/bash; do
  [ -n "${cand}" ] && [ -x "${cand}" ] || continue
  real="$(readlink -f "${cand}")"
  [ "$(stat -c %u "${real}")" = "0" ] && { BASH_BIN="${real}"; break; }
done
[ -n "${BASH_BIN}" ] || harness_error "no root-owned bash found"

SCRATCH="${TMPDIR:-${RUNNER_TEMP:-}}"
[ -n "${SCRATCH}" ] && [ -d "${SCRATCH}" ] || harness_error "set TMPDIR to a scratch directory"
WORK="$(mktemp -d "${SCRATCH}/issue-1551.XXXXXX")" || harness_error "mktemp failed"
WORK="$(cd "${WORK}" && pwd -P)"
trap 'chmod -R u+rwx "${WORK}" 2>/dev/null; rm -rf "${WORK}"' EXIT

# System tools with every gh removed, so the only gh the shim can find is ours.
TOOLBIN="${WORK}/sysbin"; mkdir -p "${TOOLBIN}"
for d in /usr/local/bin /usr/bin /bin; do
  [ -d "$d" ] || continue
  find "$d" -maxdepth 1 \( -type f -o -type l \) -perm -u+x -print0 2>/dev/null \
    | xargs -0 -r ln -s -t "${TOOLBIN}" 2>/dev/null
done
rm -f "${TOOLBIN}/gh"
[ -x "${TOOLBIN}/jq" ] || [ -x "${TOOLBIN}/bash" ] || harness_error "could not build the tool directory"

# The stub "real" gh. It logs one line per call and the last call's argv
# (NUL-separated), keeps the PR state in a file, and marks the PR MERGED when
# any merge reaches it, so a leak shows up in the state as well as the log.
STUB="${WORK}/stub"; mkdir -p "${STUB}"
{
  printf '#!%s\n' "${BASH_BIN}"
  cat <<'GH'
printf '%s\n' "$*" >> "${GH_LOG:?}"
: > "${GH_ARGV:?}"; for a in "$@"; do printf '%s\0' "$a" >> "${GH_ARGV}"; done
[ -f "${GH_STATE:?}" ] || printf 'OPEN' > "${GH_STATE}"
lc="$(printf '%s' "$*" | tr '[:upper:]' '[:lower:]')"
case "${1:-} ${2:-}" in
  "pr merge") printf 'MERGED' > "${GH_STATE}"; echo "stub: merged"; exit "${STUB_MERGE_RC:-0}" ;;
  "pr view")
    [ "${STUB_MODE:-}" = "unreadable" ] && exit 1
    case " $* " in *" --jq .url "*) printf 'https://github.com/o/r/pull/%s\n' "${3:-0}"; exit 0 ;; esac
    printf '{"state":"%s"}\n' "$(cat "${GH_STATE}")"; exit "${STUB_RC:-0}" ;;
  "alias list")
    [ -n "${STUB_ALIAS_RC:-}" ] && exit "${STUB_ALIAS_RC}"
    printf '%s' "${STUB_ALIASES:-}"; exit 0 ;;
  "pr list")
    if [ -n "${STUB_PR_LIST:-}" ]; then printf '%s\n' "${STUB_PR_LIST}"; exit 0; fi ;;
esac
if [ "${1:-}" = "api" ]; then
  case "$lc" in *merge*) printf 'MERGED' > "${GH_STATE}" ;; esac
fi
[ "${STUB_READ_STDIN:-}" = "1" ] && { printf 'stdin:'; cat; printf '\n'; }
printf 'stub-out:%s\n' "$*"
printf 'stub-err:%s\n' "$*" >&2
exit "${STUB_RC:-0}"
GH
} > "${STUB}/gh"
chmod 0755 "${STUB}/gh"

GH_LOG="${WORK}/gh.log"; GH_ARGV="${WORK}/gh.argv"; GH_STATE="${WORK}/gh.state"
OUT="${WORK}/out"; ERR="${WORK}/err"
reset_stub() { : > "${GH_LOG}"; : > "${GH_ARGV}"; printf 'OPEN' > "${GH_STATE}"; }
pr_state() { cat "${GH_STATE}"; }

# run_in <path> <cmd...>: the command in a clean environment (no inherited
# guard depth, marker, or caller PATH), stdin from /dev/null unless STDIN_FILE
# is set. Extra variables come from RUN_ENV (an array of NAME=VALUE).
RUN_ENV=()
run_in() {
  local path="$1"; shift
  local rc=0
  env -i PATH="${path}" HOME="${WORK}/home" TMPDIR="${WORK}" LC_ALL=C \
    GH_LOG="${GH_LOG}" GH_ARGV="${GH_ARGV}" GH_STATE="${GH_STATE}" \
    ${RUN_ENV[@]+"${RUN_ENV[@]}"} "$@" < "${STDIN_FILE:-/dev/null}" > "${OUT}" 2> "${ERR}" || rc=$?
  return "$rc"
}
GUARDED_PATH="${GUARD_DIR}:${STUB}:${TOOLBIN}"
# An agent's shell tool runs `gh ...` through bash, with the run's PATH.
agent_gh() { run_in "${GUARDED_PATH}" "${BASH_BIN}" -c 'gh "$@"' agent-shell "$@"; }
mkdir -p "${WORK}/home"

echo "=== #1551 no merge outside the gate ==="

# ---------------------------------------------------------------------------
# S. Static shape of the shim and the CI wiring.
# ---------------------------------------------------------------------------
echo "--- S: static ---"
if [ -f "${SHIM}" ] && [ -x "${SHIM}" ]; then
  pass "S-exists" "the shim exists and is executable"
else
  fail "S-exists" "${SHIM} is missing or not executable"
fi
# Privileged mode: no BASH_ENV, no ENV and no exported functions (SR-4).
if [ -f "${SHIM}" ] && [ "$(head -n 1 "${SHIM}")" = '#!/bin/bash -p' ]; then
  pass "S-shebang" "the shim's shebang is #!/bin/bash -p"
else
  fail "S-shebang" "the shim's first line is not '#!/bin/bash -p'"
fi
if [ -f "${SHIM}" ] && [ "$(sed -n 2p "${SHIM}")" = '# amplihack-gh-merge-guard' ]; then
  pass "S-marker-line-2" "the marker is the shim's second line"
else
  fail "S-marker-line-2" "line 2 of the shim is not '# amplihack-gh-merge-guard'"
fi
if [ -f "${SHIM}" ] && [ "$(wc -l < "${SHIM}")" -le 400 ]; then
  pass "S-budget" "the shim is within the 400-line brick budget"
else
  fail "S-budget" "the shim is missing or over 400 lines"
fi
if [ -f "${SHIM}" ] && grep -qxF '# amplihack-gh-merge-guard' "${SHIM}"; then
  pass "S-marker" "the shim carries the marker line '# amplihack-gh-merge-guard'"
else
  fail "S-marker" "the marker line '# amplihack-gh-merge-guard' is missing"
fi
if [ -f "${SHIM}" ] && ! grep -qF '/tmp' "${SHIM}"; then
  pass "S-no-tmp" "the shim names no /tmp path"
else
  fail "S-no-tmp" "the shim mentions /tmp"
fi
# Code lines only: comments may describe what the shim refuses.
SHIM_CODE="$( [ -f "${SHIM}" ] && grep -vE '^[[:space:]]*#' "${SHIM}" || true)"
if [ -n "${SHIM_CODE}" ] && ! printf '%s\n' "${SHIM_CODE}" | grep -qE '(^|[^[:alnum:]_])eval([^[:alnum:]_]|$)|(^|[^[:alnum:]_])sh -c'; then
  pass "S-no-eval" "the shim has no eval and no sh -c"
else
  fail "S-no-eval" "the shim uses eval or sh -c (or is empty)"
fi
if [ -n "${SHIM_CODE}" ] && ! printf '%s\n' "${SHIM_CODE}" | grep -qE -- '--admin|--no-verify'; then
  pass "S-no-forbidden-flags" "no forbidden flag appears in the shim's code"
else
  fail "S-no-forbidden-flags" "a forbidden flag appears in the shim's code (or it is empty)"
fi
if git -C "${REPO_ROOT}" ls-files --error-unmatch "${SHIM}" >/dev/null 2>&1; then
  if [ "$(git -C "${REPO_ROOT}" ls-files -s -- "${SHIM}" | cut -d' ' -f1)" = "100755" ]; then
    pass "S-git-mode" "git records the shim as 100755"
  else
    fail "S-git-mode" "git does not record the shim as executable (100755)"
  fi
fi
if [ -f "${CI_YML}" ] && grep -qF 'test-issue-1551-no-merge-outside-gate.sh' "${CI_YML}"; then
  pass "S-ci" "CI runs this test"
else
  fail "S-ci" ".github/workflows/ci.yml does not run test-issue-1551-no-merge-outside-gate.sh"
fi
if [ -f "${SHIM}" ] && grep -qF 'AMPLIHACK_GH_GUARD_DEPTH' "${SHIM}"; then
  pass "S-depth-var" "the shim caps re-entry with AMPLIHACK_GH_GUARD_DEPTH"
else
  fail "S-depth-var" "the shim has no AMPLIHACK_GH_GUARD_DEPTH re-entry cap"
fi

# G6. The gate itself runs privileged, and drops any gh/git/jq function it
# was handed, as a second layer behind the shim's argv and environ checks.
if [ "$(head -n 1 "${REAL_GATE}")" = '#!/bin/bash -p' ]; then
  pass "S-G6-gate-shebang" "autodrive_merge_gate.sh starts with #!/bin/bash -p"
else
  fail "S-G6-gate-shebang" "autodrive_merge_gate.sh's first line is '$(head -n 1 "${REAL_GATE}")', not '#!/bin/bash -p'"
fi
# `#!/usr/bin/env bash -p` passes "bash -p" to env as one word on Linux.
if ! grep -qE '^#!/usr/bin/env bash -p' "${REAL_GATE}" "${SHIM}"; then
  pass "S-G6-no-env-p" "neither the gate nor the shim uses '#!/usr/bin/env bash -p'"
else
  fail "S-G6-no-env-p" "a '#!/usr/bin/env bash -p' shebang never reaches bash as -p"
fi
GATE_UNSET_LINE="$(grep -nE '^[[:space:]]*builtin unset -f( [a-z]+)* gh( [a-z]+)* git( [a-z]+)* jq' "${REAL_GATE}" | awk -F: 'NR==1{print $1}')"
GATE_FIRST_GH="$(grep -nE '(^|[^[:alnum:]_./-])gh ' "${REAL_GATE}" | grep -vE '^[0-9]+:[[:space:]]*(#|builtin unset )' | awk -F: 'NR==1{print $1}')"
if [ -n "${GATE_UNSET_LINE}" ] && { [ -z "${GATE_FIRST_GH}" ] || [ "${GATE_UNSET_LINE}" -lt "${GATE_FIRST_GH}" ]; }; then
  pass "S-G6-gate-unset" "the gate runs 'builtin unset -f gh git jq' before its first gh call"
else
  fail "S-G6-gate-unset" "the gate has no 'builtin unset -f gh git jq' ahead of its first gh call (unset line: ${GATE_UNSET_LINE:-none}, first gh: ${GATE_FIRST_GH:-none})"
fi
# G12. The shim re-reads its parent pid from /proc/self/stat (field 4, after
# the last ')') and compares it with $PPID, so a reparented shim is refused.
if [ -n "${SHIM_CODE}" ] && printf '%s\n' "${SHIM_CODE}" | grep -qF '/proc/self/stat' \
   && printf '%s\n' "${SHIM_CODE}" | grep -qE '##\*\)|##\*\\\)' \
   && printf '%s\n' "${SHIM_CODE}" | grep -qE '(==|!=|-eq|-ne)[[:space:]]*"?\$\{?PPID|\$\{?PPID\}?"?[[:space:]]*(==|!=|-eq|-ne)'; then
  pass "S-G12-ppid-recheck" "the shim reads /proc/self/stat after the last ')' to re-check \$PPID"
else
  fail "S-G12-ppid-recheck" "the shim does not re-check its parent pid against /proc/self/stat"
fi
# G6. The shim reads the parent's environ.
if [ -n "${SHIM_CODE}" ] && printf '%s\n' "${SHIM_CODE}" | grep -qE '/environ'; then
  pass "S-G6-environ" "the shim reads /proc/\$PPID/environ"
else
  fail "S-G6-environ" "the shim never reads the parent's environ"
fi
# G6. No recipe or tool runs the gate as `bash "$GATE"`: without -p the shim
# refuses its merge, so such a caller would never merge.
BARE_CALLERS="$(grep -rnE '(^|[^-[:alnum:]_/])bash[[:space:]]+"?\$\{?GATE(_PHYS)?\}?"?([[:space:]]|$)' "${RECIPES}"/*.yaml "${TOOLS}"/*.sh 2>/dev/null || true)"
if [ -z "${BARE_CALLERS}" ]; then
  pass "S-G6-no-bare-callers" "no recipe or tool runs the gate as bash \"\$GATE\" without -p"
else
  fail "S-G6-no-bare-callers" "callers run the gate without -p: ${BARE_CALLERS}"
fi

if [ ! -x "${SHIM}" ]; then
  echo "  (the shim is missing: the behaviour cases below fail with it)" >&2
fi

# ---------------------------------------------------------------------------
# T1. Every merge form from a phase-1 agent's shell is refused.
# ---------------------------------------------------------------------------
echo "--- T1: merge forms are refused ---"
# refused <id> <label> <gh-args...>: exit 3, the fixed REFUSED line with that
# label, nothing merged and nothing but `alias list` reached the stub.
refused() {
  local id="$1" label="$2"; shift 2
  reset_stub
  agent_gh "$@"; local rc=$?
  local leaked
  leaked="$(grep -vxF 'alias list' "${GH_LOG}" || true)"
  if [ "$rc" -eq 3 ] \
     && grep -qF "${REFUSED_PREFIX}${label}${REFUSED_SUFFIX}" "${ERR}" \
     && [ -z "${leaked}" ] && [ "$(pr_state)" = "OPEN" ]; then
    pass "$id" "gh $* is refused as '${label}' and the PR stays OPEN"
  else
    fail "$id" "gh $*: rc=${rc} (want 3), state=$(pr_state), stub saw: ${leaked:-<nothing>}, stderr: $(head -c 300 "${ERR}")"
  fi
}
L_PR='gh pr merge'; L_API='gh api merge endpoint'; L_GQL='gh api graphql merge mutation'
L_GQL_UNREAD='gh api graphql unreadable query'; L_ALIAS='gh alias'

refused T1-pr-merge "$L_PR" pr merge 38 --squash
refused T1-repo-flag "$L_PR" -R o/r pr merge 38
refused T1-repo-long "$L_PR" --repo o/r pr merge 38
refused T1-repo-eq "$L_PR" --repo=o/r pr merge 38
refused T1-hostname "$L_PR" --hostname github.example pr merge 38
refused T1-double-dash "$L_PR" -- pr merge 38
refused T1-auto "$L_PR" pr merge 38 --auto --squash
refused T1-disable-auto "$L_PR" pr merge 38 --disable-auto
refused T1-api-X "$L_API" api -X PUT repos/o/r/pulls/38/merge
refused T1-api-method-eq "$L_API" api --method=PUT /repos/o/r/pulls/38/merge
refused T1-api-XPUT "$L_API" api -XPUT repos/o/r/pulls/38/merge
refused T1-api-no-method "$L_API" api repos/o/r/pulls/38/merge
refused T1-api-merges "$L_API" api -X POST repos/o/r/merges -f base=main -f head=feat
refused T1-api-upper "$L_API" api -X PUT REPOS/O/R/PULLS/38/MERGE
refused T1-api-url "$L_API" api -X PUT https://api.github.com/repos/o/r/pulls/38/merge
refused T1-api-ghes "$L_API" api -X PUT https://ghe.example/api/v3/repos/o/r/pulls/38/merge
refused T1-api-trailing "$L_API" api -X PUT repos/o/r/pulls/38/merge/
refused T1-api-query "$L_API" api -X PUT 'repos/o/r/pulls/38/merge?x=1'
refused T1-api-fragment "$L_API" api -X PUT 'repos/o/r/pulls/38/merge#f'
refused T1-api-doubled "$L_API" api -X PUT repos/o/r//pulls//38/merge
refused T1-api-pct "$L_API" api -X PUT repos/o%2Fr/pulls/38/merge
refused T1-api-pct-pulls "$L_API" api -X PUT repos/o/r/pull%73/38/merge
refused T1-gql-inline "$L_GQL" api graphql -f 'query=mutation { mergePullRequest(input:{pullRequestId:"PR_x"}) { clientMutationId } }'
refused T1-gql-case "$L_GQL" api graphql -f 'query=mutation { MERGEPULLREQUEST(input:{pullRequestId:"PR_x"}) { clientMutationId } }'
refused T1-gql-automerge "$L_GQL" api graphql -f 'query=mutation { enablePullRequestAutoMerge(input:{pullRequestId:"PR_x"}) { clientMutationId } }'
refused T1-gql-enqueue "$L_GQL" api graphql -f 'query=mutation { enqueuePullRequest(input:{pullRequestId:"PR_x"}) { clientMutationId } }'
GQL_FILE="${WORK}/q.graphql"; printf 'query { viewer { login } }\n' > "${GQL_FILE}"
refused T1-gql-input "$L_GQL_UNREAD" api graphql --input "${GQL_FILE}"
refused T1-gql-at-file "$L_GQL_UNREAD" api graphql -F "query=@${GQL_FILE}"
refused T1-gql-at-stdin "$L_GQL_UNREAD" api graphql -F 'query=@-'
# Every endpoint spelling that reaches GraphQL, and file reads hidden in
# combined short flags (PR #1644 review).
refused T1-gql-url-file "$L_GQL_UNREAD" api https://api.github.com/graphql -F "query=@${GQL_FILE}"
refused T1-gql-url-input "$L_GQL_UNREAD" api https://api.github.com/graphql --input "${GQL_FILE}"
refused T1-gql-ghes-file "$L_GQL_UNREAD" api https://ghe.example.com/api/graphql -F "query=@${GQL_FILE}"
refused T1-gql-ghes-path "$L_GQL_UNREAD" api /api/graphql --input "${GQL_FILE}"
refused T1-gql-query-suffix "$L_GQL_UNREAD" api 'graphql?' -F "query=@${GQL_FILE}"
refused T1-gql-slashes "$L_GQL_UNREAD" api '//graphql/' -F "query=@${GQL_FILE}"
refused T1-gql-upper "$L_GQL_UNREAD" api GRAPHQL -F "query=@${GQL_FILE}"
refused T1-gql-combined-iF "$L_GQL_UNREAD" api -iF "query=@${GQL_FILE}" graphql
refused T1-gql-combined-iF-joined "$L_GQL_UNREAD" api "-iFquery=@${GQL_FILE}" graphql
refused T1-gql-field-long "$L_GQL_UNREAD" api graphql --field "query=@${GQL_FILE}"
refused T1-gql-url-inline "$L_GQL" api https://api.github.com/graphql -f 'query=mutation { mergePullRequest(input:{pullRequestId:"PR_x"}) { clientMutationId } }'
refused T1-alias-set "$L_ALIAS" alias set pm 'pr merge'
refused T1-alias-set-gql "$L_ALIAS" alias set q 'api graphql'
ALIAS_FILE="${WORK}/aliases.yml"; printf 'pm: pr merge\n' > "${ALIAS_FILE}"
refused T1-alias-import "$L_ALIAS" alias import "${ALIAS_FILE}"
# An alias that already exists and expands to a merge form.
RUN_ENV=("STUB_ALIASES=pm: pr merge
co: pr checkout
")
refused T1-alias-existing "$L_PR" pm 38
# An extension runs its own code and can merge without calling back into the
# guard: running, installing or upgrading one is refused, and so is a word
# that is neither a built-in nor an alias, or an alias that expands to one.
L_EXT='gh extension'
refused T1-extension-direct "$L_EXT" some-extension do-thing
refused T1-extension-exec "$L_EXT" extension exec some-extension
refused T1-extension-install "$L_EXT" extension install o/gh-merger
refused T1-extension-install-ext "$L_EXT" ext install o/gh-merger
refused T1-extension-upgrade "$L_EXT" extensions upgrade --all
RUN_ENV=("STUB_ALIASES=me: some-extension do-thing")
refused T1-alias-to-extension "$L_EXT" me
RUN_ENV=()
# A `$N` placeholder fills the merge word in at call time (`pr $1` + `merge`),
# and quotes are split by gh, not the shim: both refused at set and at use.
refused T1-alias-set-placeholder "$L_ALIAS" alias set px 'pr $1'
refused T1-alias-set-shell-placeholder "$L_ALIAS" alias set --shell ps '/usr/bin/gh pr $1'
RUN_ENV=("STUB_ALIASES=px: pr \$1")
refused T1-alias-use-placeholder "$L_ALIAS" px merge 38
RUN_ENV=("STUB_ALIASES=pq: pr \"\$1\"")
refused T1-alias-use-quoted "$L_ALIAS" pq merge 38
RUN_ENV=("STUB_ALIASES=ps: '!/usr/bin/gh pr \$1'")
refused T1-alias-use-shell-placeholder "$L_ALIAS" ps merge 38
RUN_ENV=()
# GitHub serves repo routes under repositories/<id>/ too, and {branch}
# expands unescaped to the current branch, which may hold slashes.
refused T1-api-repositories-id "$L_API" api -X PUT repositories/212613049/pulls/38/merge
refused T1-api-repositories-merges "$L_API" api -X POST repositories/212613049/merges -f base=main -f head=x
refused T1-api-branch-placeholder "$L_API" api -X PUT 'repos/o/{branch}'
# `gh alias list` failing makes an unknown first word ambiguous: refused.
RUN_ENV=("STUB_ALIAS_RC=1")
reset_stub; agent_gh pm 38; rc=$?
if [ "$rc" -eq 3 ] && grep -qF "${REFUSED_PREFIX}" "${ERR}" && [ "$(pr_state)" = "OPEN" ]; then
  pass "T1-alias-list-fails" "an unknown word is refused when gh alias list fails"
else
  fail "T1-alias-list-fails" "rc=${rc} (want 3), stderr: $(head -c 300 "${ERR}")"
fi
RUN_ENV=()

# The secret in a header never reaches the refusal.
reset_stub
agent_gh api -H 'Authorization: token SECRET_1551_TOKEN' -X PUT repos/o/r/pulls/38/merge; rc=$?
if [ "$rc" -eq 3 ] && ! grep -qF 'SECRET_1551_TOKEN' "${ERR}" "${OUT}"; then
  pass "T1-no-argv-echo" "the refusal never echoes argv (a -H token stays out of the log)"
else
  fail "T1-no-argv-echo" "rc=${rc}; the refusal printed the header token or did not refuse"
fi
# The refusal is one line on stderr and nothing on stdout.
reset_stub; agent_gh pr merge 38; rc=$?
if [ "$rc" -eq 3 ] && [ ! -s "${OUT}" ] && [ "$(wc -l < "${ERR}")" -eq 1 ] \
   && grep -qF "leave the PR open" "${ERR}"; then
  pass "T1-one-line" "a refusal is one stderr line, nothing on stdout"
else
  fail "T1-one-line" "rc=${rc}, stdout bytes=$(wc -c < "${OUT}"), stderr lines=$(wc -l < "${ERR}")"
fi

# ---------------------------------------------------------------------------
# T2 / T3. The gate check: only the trusted gate, as the immediate parent.
# ---------------------------------------------------------------------------
echo "--- T2/T3: gate identity ---"
# A bundle layout of our own: tools/gh-merge-guard/gh (a copy of the shim) and
# tools/autodrive_merge_gate.sh, a stand-in gate. The shim trusts the gate
# beside its own directory, so this exercises the allow path without the
# evidence the real gate needs.
BUNDLE_TOOLS="${WORK}/bundle/amplifier-bundle/tools"
mkdir -p "${BUNDLE_TOOLS}/gh-merge-guard"
[ -f "${SHIM}" ] && cp "${SHIM}" "${BUNDLE_TOOLS}/gh-merge-guard/gh" && chmod 0755 "${BUNDLE_TOOLS}/gh-merge-guard/gh"
SHA=0123456789abcdef0123456789abcdef01234567
cat > "${BUNDLE_TOOLS}/autodrive_merge_gate.sh" <<'GATE'
# Stand-in for autodrive_merge_gate.sh: merges the way the real gate does,
# with work after the merge so bash cannot exec gh in place of itself.
set -uo pipefail
MERGE_ARGV=(pr merge 38 --squash --delete-branch --match-head-commit "${GATE_SHA}")
case "${GATE_MODE:-direct}" in
  direct) gh "${MERGE_ARGV[@]}"; rc=$? ;;
  subst)  out="$(gh "${MERGE_ARGV[@]}")"; rc=$?; printf '%s\n' "$out" ;;
  pipe)   gh "${MERGE_ARGV[@]}" | cat; rc=${PIPESTATUS[0]} ;;
  child)  "${BASH}" "${GATE_CHILD}"; rc=$? ;;
  *) rc=2 ;;
esac
echo "gate: rc=${rc}" >&2
exit "$rc"
GATE
TRUSTED_GATE="${BUNDLE_TOOLS}/autodrive_merge_gate.sh"
BUNDLE_PATH="${BUNDLE_TOOLS}/gh-merge-guard:${STUB}:${TOOLBIN}"
MERGE_LINE="pr merge 38 --squash --delete-branch --match-head-commit ${SHA}"

# allowed <id> <mode>: the trusted gate, run the way merge-loop step-03 runs
# it (`<root bash> -p <physical gate path>`, G6), merges and the stub logs the
# exact argv.
allowed() {
  local id="$1" mode="$2"
  reset_stub
  RUN_ENV=("GATE_SHA=${SHA}" "GATE_MODE=${mode}" ${ALLOWED_ENV[@]+"${ALLOWED_ENV[@]}"})
  run_in "${BUNDLE_PATH}" "${BASH_BIN}" -p "${TRUSTED_GATE}"; local rc=$?
  RUN_ENV=()
  if [ "$rc" -eq 0 ] && grep -qxF "${MERGE_LINE}" "${GH_LOG}" && ! grep -qF 'REFUSED' "${ERR}" \
     && [ "$(pr_state)" = "MERGED" ]; then
    pass "$id" "the trusted gate's merge (${mode}) reaches the real gh with its exact argv"
  else
    fail "$id" "mode=${mode}: rc=${rc}, stub log: $(tr '\n' '|' < "${GH_LOG}"), stderr: $(head -c 300 "${ERR}")"
  fi
}
ALLOWED_ENV=()
allowed T3-gate-direct direct
allowed T3-gate-subst subst
allowed T3-gate-pipe pipe
# Extra trailing arguments after the gate path are the gate's own arguments.
reset_stub
RUN_ENV=("GATE_SHA=${SHA}" "GATE_MODE=direct")
run_in "${BUNDLE_PATH}" "${BASH_BIN}" -p "${TRUSTED_GATE}" --pr 38 --repo /nowhere; rc=$?
RUN_ENV=()
if [ "$rc" -eq 0 ] && grep -qxF "${MERGE_LINE}" "${GH_LOG}"; then
  pass "T3-gate-with-args" "[bash, -p, gate, args...] is the trusted shape"
else
  fail "T3-gate-with-args" "rc=${rc}, stderr: $(head -c 300 "${ERR}")"
fi

# forged <id> <why> <cmd...>: run under the bundle PATH; the merge is refused.
forged() {
  local id="$1" why="$2"; shift 2
  reset_stub
  run_in "${BUNDLE_PATH}" "$@"; local rc=$?
  if [ "$rc" -ne 0 ] && grep -qF "${REFUSED_PREFIX}" "${ERR}" \
     && ! grep -q '^pr merge' "${GH_LOG}" && [ "$(pr_state)" = "OPEN" ]; then
    pass "$id" "$why"
  else
    fail "$id" "${why} -- not refused: rc=${rc}, stub log: $(tr '\n' '|' < "${GH_LOG}"), stderr: $(head -c 300 "${ERR}")"
  fi
}
forged T2-bash-c-text "bash -c text naming the gate is not the gate" \
  "${BASH_BIN}" -c 'echo autodrive_merge_gate.sh; gh pr merge 1; exit $?'
FAKE_DIR="${WORK}/fake"; mkdir -p "${FAKE_DIR}"
printf 'gh pr merge 1 --squash\nexit $?\n' > "${FAKE_DIR}/autodrive_merge_gate.sh"
forged T2-fake-gate "a different file named autodrive_merge_gate.sh is refused" \
  "${BASH_BIN}" "${FAKE_DIR}/autodrive_merge_gate.sh"
# A byte-identical copy of the trusted gate, with an evil helper beside it.
COPY_DIR="${WORK}/copy"; mkdir -p "${COPY_DIR}"
cp "${TRUSTED_GATE}" "${COPY_DIR}/autodrive_merge_gate.sh"
printf 'block() { :; }\n' > "${COPY_DIR}/autodrive_state.sh"
RUN_ENV=("GATE_SHA=${SHA}")
forged T2-gate-copy "a byte-identical copy of the gate elsewhere is refused" \
  "${BASH_BIN}" "${COPY_DIR}/autodrive_merge_gate.sh"
RUN_ENV=()
# argv[0] naming the gate is never trusted.
forged T2-exec-a "exec -a <gate> with another script is refused" \
  "${BASH_BIN}" -c 'exec -a "$1" "$2" "$3"' x "${TRUSTED_GATE}" "${BASH_BIN}" "${FAKE_DIR}/autodrive_merge_gate.sh"
# argv[0] with an embedded newline then the gate path: a newline-split parser
# would read the gate as argv[1].
forged T2-exec-a-newline "an argv[0] of 'bash<newline><gate>' is refused" \
  "${BASH_BIN}" -c 'exec -a "$1" "$2" "$3"' x "bash
${TRUSTED_GATE}" "${BASH_BIN}" "${FAKE_DIR}/autodrive_merge_gate.sh"
# The trusted gate run as `bash -c` text with the gate path as $0.
forged T2-gate-as-dollar0 "bash -c with the gate path as \$0 is refused" \
  "${BASH_BIN}" -c 'gh pr merge 1; exit $?' "${TRUSTED_GATE}"
# A program the gate runs is a grandchild of the gate: never trusted.
CHILD="${WORK}/child.sh"; printf 'gh pr merge 38 --squash\nexit $?\n' > "${CHILD}"
RUN_ENV=("GATE_SHA=${SHA}" "GATE_MODE=child" "GATE_CHILD=${CHILD}")
forged T2-gate-child "a script the gate runs cannot merge (immediate parent only)" \
  "${BASH_BIN}" -p "${TRUSTED_GATE}"
RUN_ENV=()
# G6. The argv shape is exactly [<root bash>, -p, <physical gate>, ...].
# The trusted gate without -p: a BASH_ENV or exported function would run
# inside it, so its merge is refused.
RUN_ENV=("GATE_SHA=${SHA}" "GATE_MODE=direct")
forged T2-G6-no-p "the trusted gate run without -p is refused" \
  "${BASH_BIN}" "${TRUSTED_GATE}"
# -p in another position, or as part of a combined flag word, is not the shape.
forged T2-G6-p-after-gate "-p after the gate path is refused" \
  "${BASH_BIN}" "${TRUSTED_GATE}" -p
forged T2-G6-combined-flags "a combined '-pe' flag word is refused" \
  "${BASH_BIN}" -pe "${TRUSTED_GATE}"
forged T2-G6-extra-flag "an extra interpreter flag before the gate is refused" \
  "${BASH_BIN}" -p -x "${TRUSTED_GATE}"
forged T2-G6-bash-c-p "bash -p -c naming the gate is refused" \
  "${BASH_BIN}" -p -c 'gh pr merge 38; exit $?' "${TRUSTED_GATE}"
# argv[2] must be the physical path itself: a symlink to the gate, a relative
# path and a path with a dot segment are not it (step-03 passes pwd -P's).
ln -s "${TRUSTED_GATE}" "${WORK}/gate-link.sh"
forged T2-G6-symlink "the gate through a symlink is refused (argv[2] must be the physical path)" \
  "${BASH_BIN}" -p "${WORK}/gate-link.sh"
forged T2-G6-relative "the gate by a relative path is refused" \
  "${BASH_BIN}" -c 'cd "$1" && exec "$2" -p ./autodrive_merge_gate.sh' x "${BUNDLE_TOOLS}" "${BASH_BIN}"
forged T2-G6-dot-segment "the gate by a path with a dot segment is refused" \
  "${BASH_BIN}" -p "${BUNDLE_TOOLS}/gh-merge-guard/../autodrive_merge_gate.sh"
RUN_ENV=()
# A non-merge call from a forged gate still passes through.
reset_stub
run_in "${BUNDLE_PATH}" "${BASH_BIN}" -c 'gh pr view 38; exit $?' "${TRUSTED_GATE}"; rc=$?
if [ "$rc" -eq 0 ] && grep -qxF 'pr view 38' "${GH_LOG}"; then
  pass "T2-non-merge-anywhere" "a non-merge call passes through whoever the parent is"
else
  fail "T2-non-merge-anywhere" "rc=${rc}, stderr: $(head -c 300 "${ERR}")"
fi

# The real gate under the repository's own guard: its NOT_MERGED path (PR
# metadata unreadable) still exits 1 with NOT_MERGED, and nothing is refused.
REPO_FX="${WORK}/repo"; mkdir -p "${REPO_FX}"
reset_stub
RUN_ENV=("STUB_MODE=unreadable")
run_in "${GUARDED_PATH}" "${BASH_BIN}" "${REAL_GATE}" --pr 38 --repo "${REPO_FX}" --state-dir "${WORK}/gate-state"; rc=$?
RUN_ENV=()
if [ "$rc" -eq 1 ] && grep -qF '"merge_result":"NOT_MERGED"' "${OUT}" && ! grep -qF 'REFUSED' "${ERR}" \
   && grep -q '^pr view 38' "${GH_LOG}"; then
  pass "T3-real-gate-not-merged" "the real gate under the guard still exits 1 with NOT_MERGED"
else
  fail "T3-real-gate-not-merged" "rc=${rc} (want 1), stdout: $(head -c 300 "${OUT}"), stderr: $(head -c 300 "${ERR}")"
fi
# The real gate's full MERGED path is exercised under the guard by
# test-auto-drive-to-merge.sh, which has the evidence harness it needs.
if grep -qF 'gh-merge-guard' "${SCRIPT_DIR}/test-auto-drive-to-merge.sh" \
   && grep -qF '1551-gate-merges-under-guard' "${SCRIPT_DIR}/test-auto-drive-to-merge.sh"; then
  pass "T3-real-gate-merged-covered" "test-auto-drive-to-merge.sh runs the real gate's merge under the guard"
else
  fail "T3-real-gate-merged-covered" "test-auto-drive-to-merge.sh has no 1551-gate-merges-under-guard case"
fi

# step-03 runs the gate by an absolute, physical path, so argv[1] is what the
# shim compares.
MERGE_LOOP="${RECIPES}/autodrive-merge-loop.yaml"
STEP03="$(awk '/^  - id: "step-03-merge-gate"/{f=1;print;next} f&&/^  - id: /{exit} f' "${MERGE_LOOP}")"
if printf '%s' "${STEP03}" | grep -qF 'pwd -P' \
   && printf '%s' "${STEP03}" | grep -qE 'GATE_PHYS="\$\(cd "\$\(dirname "\$GATE"\)" && pwd -P\)/\$\(basename "\$GATE"\)"'; then
  pass "T3-step03-absolute" "merge-loop step-03 makes the gate path absolute and physical in GATE_PHYS"
else
  fail "T3-step03-absolute" "merge-loop step-03 does not set GATE_PHYS=\"\$(cd \"\$(dirname \"\$GATE\")\" && pwd -P)/\$(basename \"\$GATE\")\""
fi
if printf '%s' "${STEP03}" | grep -qE '^[[:space:]]*exec env -u LD_PRELOAD -u LD_AUDIT -u LD_LIBRARY_PATH /bin/bash -p "\$GATE_PHYS" "\$\{ARGS\[@\]\}" \)' \
   && ! printf '%s' "${STEP03}" | grep -qE '(^|[[:space:]])bash "\$GATE"'; then
  pass "T3-G6-step03-privileged" "merge-loop step-03 runs the gate as /bin/bash -p \"\$GATE_PHYS\" with the LD_* variables stripped"
else
  fail "T3-G6-step03-privileged" "merge-loop step-03 does not run 'exec env -u LD_PRELOAD -u LD_AUDIT -u LD_LIBRARY_PATH /bin/bash -p \"\$GATE_PHYS\" \"\${ARGS[@]}\" )' (or still runs bash \"\$GATE\")"
fi

# ---------------------------------------------------------------------------
# T4. Everything else passes through byte for byte.
# ---------------------------------------------------------------------------
echo "--- T4: pass-through parity ---"
# parity <id> <gh-args...>: the same call through the shim and straight to the
# stub gives the same argv, stdout, stderr and exit code.
parity() {
  local id="$1"; shift
  reset_stub
  agent_gh "$@"; local rc_g=$?
  cp "${OUT}" "${WORK}/g.out"; cp "${ERR}" "${WORK}/g.err"; cp "${GH_ARGV}" "${WORK}/g.argv"
  reset_stub
  run_in "${STUB}:${TOOLBIN}" "${BASH_BIN}" -c 'gh "$@"' agent-shell "$@"; local rc_d=$?
  if [ "$rc_g" -eq "$rc_d" ] && cmp -s "${WORK}/g.out" "${OUT}" && cmp -s "${WORK}/g.err" "${ERR}" \
     && cmp -s "${WORK}/g.argv" "${GH_ARGV}" && [ -s "${GH_ARGV}" ]; then
    pass "$id" "gh $* passes through unchanged (rc=${rc_g})"
  else
    fail "$id" "gh $*: guarded rc=${rc_g} direct rc=${rc_d}; guarded stderr: $(head -c 300 "${WORK}/g.err")"
  fi
}
parity T4-pr-view pr view 38 --json state
parity T4-pr-create pr create --title 't' --body 'b "q" $(x)' --base main
parity T4-pr-checks pr checks 38 --json name,state,bucket
parity T4-api-pull api repos/o/r/pulls/38
parity T4-api-files api repos/o/r/pulls/38/files
parity T4-api-merge-something api repos/o/r/pulls/38/merge-something
parity T4-api-branch-merge-word api repos/o/r/branches/merge-train
parity T4-gql-query api graphql --paginate -F pr=38 -F owner='{owner}' -F name='{repo}' -f 'query=query($pr:Int!){ repository { pullRequest(number:$pr) { mergeable } } }'
parity T4-repo-flag -R o/r pr view 38
parity T4-pr-list pr list --state all --limit 1
parity T4-auth-status auth status
RUN_ENV=("STUB_RC=4")
parity T4-nonzero-rc pr view 38
parity T4-nonzero-rc-api api repos/o/r/pulls/38
# Built-ins and their aliases that gh itself defines still pass through.
parity T4-discussion discussion list
parity T4-codespace-alias cs list
parity T4-ruleset-alias rs list
parity T4-actions-topic actions
parity T4-repositories-get api repositories/212613049/pulls/38
parity T4-ext-list extension list
parity T4-ext-remove ext remove some-ext
RUN_ENV=()
# stdin reaches the real gh.
printf 'body from stdin\n' > "${WORK}/stdin.txt"
RUN_ENV=("STUB_READ_STDIN=1")
reset_stub; STDIN_FILE="${WORK}/stdin.txt" agent_gh pr create --body-file -; rc=$?
RUN_ENV=()
unset STDIN_FILE
if [ "$rc" -eq 0 ] && grep -qF 'stdin:body from stdin' "${OUT}"; then
  pass "T4-stdin" "stdin passes through to the real gh"
else
  fail "T4-stdin" "rc=${rc}, stdout: $(head -c 300 "${OUT}")"
fi
# Argument bytes survive: spaces, quotes, globs, newlines, a leading dash.
reset_stub
agent_gh issue comment 1 --body $'line one\n  *glob* "dq" \'sq\' $HOME\n-dash'; rc=$?
printf '%s\0' issue comment 1 --body $'line one\n  *glob* "dq" \'sq\' $HOME\n-dash' > "${WORK}/want.argv"
if [ "$rc" -eq 0 ] && cmp -s "${WORK}/want.argv" "${GH_ARGV}"; then
  pass "T4-argv-bytes" "argv reaches the real gh byte for byte"
else
  fail "T4-argv-bytes" "rc=${rc}; argv changed on the way through"
fi

# ---------------------------------------------------------------------------
# T5. No real gh, or a loop back into the shim: exit 127, never a hang.
# ---------------------------------------------------------------------------
echo "--- T5: finding the real gh ---"
reset_stub
run_in "${GUARD_DIR}:${TOOLBIN}" "${BASH_BIN}" -c 'gh "$@"' s pr view 38; rc=$?
if [ "$rc" -eq 127 ] && grep -qF 'amplihack gh-merge-guard: real gh not found on PATH' "${ERR}"; then
  pass "T5-no-real-gh" "no real gh on PATH: exit 127 with the fixed message"
else
  fail "T5-no-real-gh" "rc=${rc} (want 127), stderr: $(head -c 300 "${ERR}")"
fi
G2="${WORK}/guard2"; mkdir -p "${G2}"
[ -f "${SHIM}" ] && cp "${SHIM}" "${G2}/gh" && chmod 0755 "${G2}/gh"
reset_stub
run_in "${GUARD_DIR}:${G2}:${TOOLBIN}" "${BASH_BIN}" -c 'gh "$@"' s pr view 38; rc=$?
if [ "$rc" -eq 127 ]; then
  pass "T5-two-shims" "a second shim copy is skipped by its marker, not run as the real gh"
else
  fail "T5-two-shims" "rc=${rc} (want 127), stderr: $(head -c 300 "${ERR}")"
fi
# Two shims in front of the real gh: the call still reaches it.
reset_stub
run_in "${GUARD_DIR}:${G2}:${STUB}:${TOOLBIN}" "${BASH_BIN}" -c 'gh "$@"' s pr view 38; rc=$?
if [ "$rc" -eq 0 ] && grep -qxF 'pr view 38' "${GH_LOG}"; then
  pass "T5-two-shims-then-real" "two shim copies in front of the real gh still reach it once"
else
  fail "T5-two-shims-then-real" "rc=${rc}, stub log: $(tr '\n' '|' < "${GH_LOG}")"
fi
# Another wrapper that calls `gh` again through PATH.
WRAP="${WORK}/wrap"; mkdir -p "${WRAP}"
printf '#!%s\nexec gh "$@"\n' "${BASH_BIN}" > "${WRAP}/gh"; chmod 0755 "${WRAP}/gh"
reset_stub
run_in "${GUARD_DIR}:${WRAP}:${TOOLBIN}" "${TOOLBIN}/timeout" 60 "${BASH_BIN}" -c 'gh "$@"' s pr view 38; rc=$?
if [ "$rc" -eq 127 ]; then
  pass "T5-reentry-cap" "a wrapper that re-enters the shim ends with 127, not a hang"
else
  fail "T5-reentry-cap" "rc=${rc} (want 127; 124 means it hung), stderr: $(head -c 300 "${ERR}")"
fi
# A deeply inherited depth can only fail, never permit.
reset_stub
RUN_ENV=("AMPLIHACK_GH_GUARD_DEPTH=99")
agent_gh pr view 38; rc=$?
RUN_ENV=()
if [ "$rc" -eq 127 ] && [ ! -s "${GH_LOG}" ]; then
  pass "T5-inherited-depth" "an inherited depth over the cap fails with 127"
else
  fail "T5-inherited-depth" "rc=${rc} (want 127)"
fi
# A user's own wrapper after the shim is called like any gh.
UWRAP="${WORK}/userwrap"; mkdir -p "${UWRAP}"
printf '#!%s\nprintf "userwrap\\n" >> "%s"\nexec "%s" "$@"\n' "${BASH_BIN}" "${WORK}/userwrap.log" "${STUB}/gh" > "${UWRAP}/gh"
chmod 0755 "${UWRAP}/gh"
reset_stub
run_in "${GUARD_DIR}:${UWRAP}:${STUB}:${TOOLBIN}" "${BASH_BIN}" -c 'gh "$@"' s pr view 38; rc=$?
if [ "$rc" -eq 0 ] && [ -s "${WORK}/userwrap.log" ] && grep -qxF 'pr view 38' "${GH_LOG}"; then
  pass "T5-user-wrapper" "a user's wrapper after the shim is called as the next gh"
else
  fail "T5-user-wrapper" "rc=${rc}, stderr: $(head -c 300 "${ERR}")"
fi
# The shim's own directory is skipped even when PATH names it twice.
reset_stub
run_in "${GUARD_DIR}:${GUARD_DIR}/:${STUB}:${TOOLBIN}" "${BASH_BIN}" -c 'gh "$@"' s pr view 38; rc=$?
if [ "$rc" -eq 0 ] && [ "$(grep -cxF 'pr view 38' "${GH_LOG}")" -eq 1 ]; then
  pass "T5-self-twice" "the shim's directory named twice on PATH is skipped both times"
else
  fail "T5-self-twice" "rc=${rc}, stderr: $(head -c 300 "${ERR}")"
fi

# ---------------------------------------------------------------------------
# SEC. Security regressions (SEC-1 to SEC-7).
# ---------------------------------------------------------------------------
echo "--- SEC: security regressions ---"
# SEC-1. A flag consumes the next argument only when it is an exact token that
# takes a value; attached and unknown flags are one token each.
refused SEC1-XPUT-then-f "$L_API" api -XPUT -f commit_title=t repos/o/r/pulls/38/merge
refused SEC1-unknown-eq "$L_API" api --method=PUT --unknown-1551=y repos/o/r/pulls/38/merge
refused SEC1-header "$L_API" api -H 'Accept: application/vnd.github+json' -X PUT repos/o/r/pulls/38/merge
refused SEC1-jq "$L_API" api --jq .merged -XPUT repos/o/r/pulls/38/merge
refused SEC1-endpoint-first "$L_API" api repos/o/r/pulls/38/merge -XPUT -f merge_method=squash
refused SEC1-attached-f "$L_API" api -fcommit_title=t -XPUT repos/o/r/pulls/38/merge
printf '{}\n' > "${WORK}/body.json"
refused SEC1-input-rest "$L_API" api --input "${WORK}/body.json" -X PUT repos/o/r/pulls/38/merge
refused SEC1-hostname-eq "$L_PR" --hostname=github.example pr merge 38

# SEC-2. Endpoints that mention pull or merge with dot segments, a backslash,
# whitespace or a control character are refused, not resolved.
refused SEC2-dot "$L_API" api -X PUT repos/o/r/pulls/38/./merge
refused SEC2-dotdot "$L_API" api -X PUT repos/o/r/x/../pulls/38/merge
refused SEC2-leading-dot "$L_API" api -X PUT ./repos/o/r/pulls/38/merge
refused SEC2-trailing-dotdot "$L_API" api -X PUT repos/o/r/pulls/38/merge/..
refused SEC2-backslash "$L_API" api -X PUT 'repos\o\r\pulls\38\merge'
refused SEC2-space "$L_API" api -X PUT 'repos/o/r/pulls/38/merge '
refused SEC2-tab "$L_API" api -X PUT $'repos/o/r/pulls/38\t/merge'
refused SEC2-ctrl "$L_API" api -X PUT $'repos/o/r/pulls/38/merge\x01'
refused SEC2-newline "$L_API" api -X PUT $'repos/o/r/pulls/38/merge\nx'
# Mutation names are whole names, in any case, wherever they sit in gh api.
refused SEC2-gql-alias "$L_GQL" api graphql -f 'query=mutation{m:mergePullRequest(input:{pullRequestId:"PR_x"}){clientMutationId}}'
refused SEC2-gql-F "$L_GQL" api graphql -F 'query=mutation($i:ID!){mergePullRequest(input:{pullRequestId:$i}){clientMutationId}}' -F i=PR_x
refused SEC2-gql-lower "$L_GQL" api graphql -f 'query=mutation{ enqueuepullrequest(input:{pullRequestId:"PR_x"}){clientMutationId} }'
# mergeBranch merges a head into a base; with the PR branch as head GitHub
# marks the PR merged, like the REST merges endpoint.
refused SEC2-gql-mergebranch "$L_GQL" api graphql -f 'query=mutation{mergeBranch(input:{repositoryId:"R_x",base:"main",head:"feat"}){clientMutationId}}'
refused SEC2-gql-mergebranch-case "$L_GQL" api graphql -f 'query=mutation{ MERGEBRANCH(input:{repositoryId:"R_x",base:"main",head:"feat"}){clientMutationId} }'
# The over-refusal stays narrow: no pull or merge, or not gh api, passes.
parity SEC2-dotdot-no-merge api repos/o/r/contents/a/../b.md
parity SEC2-gql-longer-name api graphql -f 'query=query { mergePullRequestX }'
parity SEC2-body-not-api pr create --title t --body 'see mergePullRequest and repos/o/r/pulls/38/merge'
parity SEC2-issue-body issue create --title t --body 'the pulls/38/merge endpoint'

# IV-2. REST forms gh accepts beyond the plain relative path: full URLs, the
# GHES api/v3/ prefix, a leading '/', a query or fragment, gh's {owner} and
# {repo} placeholders, and any pulls/merge/merges segment not all lower-case.
refused IV2-url-upper-scheme "$L_API" api -X PUT HTTPS://API.GITHUB.COM/repos/o/r/pulls/38/merge
refused IV2-url-port "$L_API" api -X PUT https://api.github.com:443/repos/o/r/pulls/38/merge
refused IV2-url-merges "$L_API" api -X POST https://api.github.com/repos/o/r/merges -f base=main -f head=f
refused IV2-v3-relative "$L_API" api -X PUT api/v3/repos/o/r/pulls/38/merge
refused IV2-v3-leading "$L_API" api -X PUT /api/v3/repos/o/r/merges
refused IV2-v3-upper "$L_API" api -X PUT API/V3/repos/o/r/pulls/38/merge
refused IV2-leading-slash "$L_API" api -X PUT /repos/o/r/pulls/38/merge
refused IV2-leading-slashes "$L_API" api -X PUT //repos/o/r/pulls/38/merge
refused IV2-empty-query "$L_API" api -X PUT 'repos/o/r/pulls/38/merge?'
refused IV2-query-merges "$L_API" api -X POST 'repos/o/r/merges?x=1'
refused IV2-fragment-merges "$L_API" api -X POST 'repos/o/r/merges#x'
refused IV2-query-then-fragment "$L_API" api -X PUT 'repos/o/r/pulls/38/merge?a=b#c'
refused IV2-placeholders "$L_API" api -X PUT 'repos/{owner}/{repo}/pulls/38/merge'
refused IV2-placeholders-merges "$L_API" api -X POST 'repos/{owner}/{repo}/merges'
refused IV2-placeholders-leading "$L_API" api -X PUT '/repos/{owner}/{repo}/pulls/38/merge'
refused IV2-placeholder-number "$L_API" api -X PUT 'repos/{owner}/{repo}/pulls/{pr}/merge'
refused IV2-mixed-Pulls "$L_API" api -X PUT repos/o/r/Pulls/38/merge
refused IV2-mixed-Merge "$L_API" api -X PUT repos/o/r/pulls/38/Merge
refused IV2-mixed-Merges "$L_API" api -X POST repos/o/r/Merges
refused IV2-mixed-mErGe "$L_API" api -X PUT repos/o/r/pulls/38/mErGe
refused IV2-upper-PULLS "$L_API" api -X PUT repos/o/r/PULLS/38/merge
refused IV2-mixed-Repos "$L_API" api -X PUT Repos/o/r/pulls/38/merge
# A merge endpoint given as gh's positional after --method in long form.
refused IV2-method-long "$L_API" api --method PUT 'repos/{owner}/{repo}/pulls/38/merge'
# Still narrow: reads of the same PR and other placeholder paths pass through.
parity IV2-placeholder-read api 'repos/{owner}/{repo}/pulls/38'
parity IV2-placeholder-files api 'repos/{owner}/{repo}/pulls/38/files'
parity IV2-url-read api https://api.github.com/repos/o/r/pulls/38
parity IV2-leading-slash-read api /repos/o/r/pulls/38/reviews
parity IV2-merge-commit-read api 'repos/{owner}/{repo}/commits/abc123'

# SEC-3. Only absolute PATH entries are searched: a hostile ./gh in the
# working directory (an empty entry, `.`, or a relative directory) never runs.
HOSTILE="${WORK}/hostile"; mkdir -p "${HOSTILE}/rel"
for h in "${HOSTILE}/gh" "${HOSTILE}/rel/gh"; do
  printf '#!%s\n: > "%s"\nprintf MERGED > "%s"\nexit 0\n' "${BASH_BIN}" "${WORK}/pwned-gh" "${GH_STATE}" > "$h"
  chmod 0755 "$h"
done
rm -f "${WORK}/pwned-gh"; reset_stub
( cd "${HOSTILE}" && run_in "${GUARD_DIR}::.:rel:${STUB}:${TOOLBIN}" "${BASH_BIN}" -c 'gh "$@"' s pr view 38 ); rc=$?
if [ "$rc" -eq 0 ] && [ ! -e "${WORK}/pwned-gh" ] && grep -qxF 'pr view 38' "${GH_LOG}" && [ "$(pr_state)" = "OPEN" ]; then
  pass "SEC3-relative-skipped" "empty, . and relative PATH entries are skipped; the absolute real gh runs"
else
  fail "SEC3-relative-skipped" "rc=${rc}, hostile ran: $([ -e "${WORK}/pwned-gh" ] && echo yes || echo no), stderr: $(head -c 300 "${ERR}")"
fi
rm -f "${WORK}/pwned-gh"; reset_stub
( cd "${HOSTILE}" && run_in "${GUARD_DIR}::.:rel:${TOOLBIN}" "${BASH_BIN}" -c 'gh "$@"' s pr view 38 ); rc=$?
if [ "$rc" -eq 127 ] && [ ! -e "${WORK}/pwned-gh" ]; then
  pass "SEC3-relative-only" "with only a relative ./gh behind it the shim exits 127 and never runs it"
else
  fail "SEC3-relative-only" "rc=${rc} (want 127), hostile ran: $([ -e "${WORK}/pwned-gh" ] && echo yes || echo no)"
fi

# SEC-4. The inherited depth is matched against digits before any arithmetic,
# so an array subscript cannot run a command.
n=0
for v in 'x[$(touch '"${WORK}"'/pwned-depth)]' 'a[$(touch '"${WORK}"'/pwned-depth)]+0' \
         '1;touch '"${WORK}"'/pwned-depth' '-1' '1234' ' 1' '0x1' '1 '; do
  n=$((n + 1)); rm -f "${WORK}/pwned-depth"; reset_stub
  RUN_ENV=("AMPLIHACK_GH_GUARD_DEPTH=${v}")
  agent_gh pr view 38; rc=$?
  RUN_ENV=()
  if [ "$rc" -eq 127 ] && [ ! -e "${WORK}/pwned-depth" ] && [ ! -s "${GH_LOG}" ]; then
    pass "SEC4-depth-${n}" "a depth of '${v}' exits 127 and runs nothing"
  else
    fail "SEC4-depth-${n}" "depth '${v}': rc=${rc} (want 127), command ran: $([ -e "${WORK}/pwned-depth" ] && echo yes || echo no), stub log: $(tr '\n' '|' < "${GH_LOG}")"
  fi
done
reset_stub
RUN_ENV=("AMPLIHACK_GH_GUARD_DEPTH=0")
agent_gh pr view 38; rc=$?
RUN_ENV=()
if [ "$rc" -eq 0 ] && grep -qxF 'pr view 38' "${GH_LOG}"; then
  pass "SEC4-depth-zero" "a depth of 0 passes through"
else
  fail "SEC4-depth-zero" "rc=${rc}, stderr: $(head -c 300 "${ERR}")"
fi

# SEC-5. Exported functions and BASH_ENV cannot answer the gate identity
# checks: a forged gate stays refused (privileged mode, #!/bin/bash -p).
LIE="() { printf '%s\\n' '${TRUSTED_GATE}'; }"
RUN_ENV=("BASH_FUNC_realpath%%=${LIE}" "BASH_FUNC_readlink%%=${LIE}"
         "BASH_FUNC_basename%%=() { printf 'bash\\n'; }" "BASH_FUNC_stat%%=() { printf '0\\n'; }"
         "BASH_FUNC_test%%=() { return 0; }" "BASH_FUNC_builtin%%=() { return 0; }")
forged SEC5-exported-functions "exported realpath, readlink, stat, test and builtin functions do not make a forged gate trusted" \
  "${BASH_BIN}" "${FAKE_DIR}/autodrive_merge_gate.sh"
RUN_ENV=()
cat > "${WORK}/bash_env.sh" <<ENVF
printf '%s\n' "\$0" >> "${WORK}/bash_env.log"
realpath() { printf '%s\n' '${TRUSTED_GATE}'; }
readlink() { printf '%s\n' '${TRUSTED_GATE}'; }
ENVF
: > "${WORK}/bash_env.log"
RUN_ENV=("BASH_ENV=${WORK}/bash_env.sh" "ENV=${WORK}/bash_env.sh")
forged SEC5-bash-env "a BASH_ENV file defining realpath does not make a forged gate trusted" \
  "${BASH_BIN}" "${FAKE_DIR}/autodrive_merge_gate.sh"
RUN_ENV=()
if ! grep -q 'gh-merge-guard/gh$' "${WORK}/bash_env.log"; then
  pass "SEC5-bash-env-not-sourced" "the shim never sources BASH_ENV"
else
  fail "SEC5-bash-env-not-sourced" "BASH_ENV ran inside the shim: $(tr '\n' '|' < "${WORK}/bash_env.log")"
fi
# G6. The trusted gate itself, run exactly as step-03 runs it, is refused
# when its environ carries anything bash would act on at startup: an exported
# function (`export -f gh` merged with --admin in the probe), BASH_ENV, ENV,
# SHELLOPTS, BASHOPTS or PS4. -p already ignores them; refusing them as well
# means a bash without -p semantics, or a later change to the gate, cannot
# reopen the bypass.
gate_env_refused() { # gate_env_refused <id> <why> <NAME=VALUE...>
  local id="$1" why="$2"; shift 2
  RUN_ENV=("GATE_SHA=${SHA}" "GATE_MODE=direct" "$@")
  forged "$id" "$why" "${BASH_BIN}" -p "${TRUSTED_GATE}"
  RUN_ENV=()
}
ADMIN_GH="() { command gh pr merge 38 --admin --squash; }"
gate_env_refused SEC5-G6-export-f-gh "the gate with an exported gh function in its environ is refused" \
  "BASH_FUNC_gh%%=${ADMIN_GH}"
gate_env_refused SEC5-G6-export-f-other "any exported function (BASH_FUNC_*) in the gate's environ is refused" \
  "BASH_FUNC_realpath%%=() { printf '/nowhere\\n'; }"
gate_env_refused SEC5-G6-bash-env "the gate with BASH_ENV set is refused" \
  "BASH_ENV=${WORK}/bash_env.sh"
gate_env_refused SEC5-G6-env "the gate with ENV set is refused" \
  "ENV=${WORK}/bash_env.sh"
gate_env_refused SEC5-G6-shellopts "the gate with SHELLOPTS in its environ is refused" \
  "SHELLOPTS=xtrace"
gate_env_refused SEC5-G6-bashopts "the gate with BASHOPTS in its environ is refused" \
  "BASHOPTS=extdebug"
gate_env_refused SEC5-G6-ps4 "the gate with PS4 in its environ is refused" \
  'PS4=$(touch '"${WORK}"'/pwned-ps4)'
# The loader honours these despite -p: a preload can rewrite the gate's gh
# call into a merge with --admin. Setting them is enough to be refused.
gate_env_refused SEC5-G6-ld-preload "the gate with LD_PRELOAD in its environ is refused" \
  "LD_PRELOAD=${WORK}/no-such-preload.so"
gate_env_refused SEC5-G6-ld-audit "the gate with LD_AUDIT in its environ is refused" \
  "LD_AUDIT=${WORK}/no-such-audit.so"
gate_env_refused SEC5-G6-ld-library-path "the gate with LD_LIBRARY_PATH in its environ is refused" \
  "LD_LIBRARY_PATH=${WORK}/no-such-libdir"
if [ ! -e "${WORK}/pwned-ps4" ]; then
  pass "SEC5-G6-ps4-not-run" "a PS4 in the gate's environ never ran"
else
  fail "SEC5-G6-ps4-not-run" "PS4 from the environment was expanded"
fi
# The probe that merged before G6, end to end: `export -f gh` with a function
# that merges with --admin, then the gate run without -p. Refused, PR OPEN,
# and no merge (with or without --admin) reached the real gh.
reset_stub
RUN_ENV=("GATE_SHA=${SHA}" "GATE_MODE=direct" "BASH_FUNC_gh%%=${ADMIN_GH}")
run_in "${BUNDLE_PATH}" "${BASH_BIN}" "${TRUSTED_GATE}"; rc=$?
RUN_ENV=()
if [ "$rc" -ne 0 ] && ! grep -q 'merge' "${GH_LOG}" && [ "$(pr_state)" = "OPEN" ]; then
  pass "SEC5-G6-probe-export-f" "the export -f gh probe that merged with --admin is refused"
else
  fail "SEC5-G6-probe-export-f" "rc=${rc}, state=$(pr_state), stub log: $(tr '\n' '|' < "${GH_LOG}")"
fi
# The BASH_ENV probe: BASH_ENV defines gh as an --admin merge, gate run
# without -p.
printf 'gh() { command gh pr merge 38 --admin --squash; }\n' > "${WORK}/bash_env_gh.sh"
reset_stub
RUN_ENV=("GATE_SHA=${SHA}" "GATE_MODE=direct" "BASH_ENV=${WORK}/bash_env_gh.sh")
run_in "${BUNDLE_PATH}" "${BASH_BIN}" "${TRUSTED_GATE}"; rc=$?
RUN_ENV=()
if [ "$rc" -ne 0 ] && ! grep -q 'merge' "${GH_LOG}" && [ "$(pr_state)" = "OPEN" ]; then
  pass "SEC5-G6-probe-bash-env" "the BASH_ENV probe that merged with --admin is refused"
else
  fail "SEC5-G6-probe-bash-env" "rc=${rc}, state=$(pr_state), stub log: $(tr '\n' '|' < "${GH_LOG}")"
fi
# Empty values are not set values: the gate still merges with BASH_ENV= and
# PS4= present but empty (only non-empty values are refused).
ALLOWED_ENV=("BASH_ENV=" "ENV=" "PS4=")
allowed SEC5-G6-empty-values-ok direct
ALLOWED_ENV=()
# An unrelated variable whose value mentions BASH_FUNC_ is not an exported
# function.
ALLOWED_ENV=("NOTE=BASH_FUNC_gh%%=() { :; }")
allowed SEC5-G6-value-mention-ok direct
ALLOWED_ENV=()

# G6. A legitimate session (Fedora's exported `which`, Lmod's `module`, an
# exported SHELLOPTS, a CUDA LD_LIBRARY_PATH) still merges through step-03's
# own launch, which clears them before the gate starts. The same session
# launching the gate bare is refused, so the clearing is what makes it pass.
LAUNCH="$(printf '%s\n' "${STEP03}" | awk '/^[[:space:]]*\( unset -f /{f=1} f{print} f&&/"\$\{ARGS\[@\]\}" \)[[:space:]]*$/{exit}')"
LEGIT_ENV=("GATE_SHA=${SHA}" "GATE_MODE=direct" "GATE_PHYS=${TRUSTED_GATE}"
           "BASH_FUNC_which%%=() { command which \"\$@\"; }" "BASH_FUNC_module%%=() { :; }"
           "SHELLOPTS=braceexpand:hashall:interactive-comments:pipefail"
           "LD_LIBRARY_PATH=${WORK}/no-such-cuda/lib64")
legit_launch() { # legit_launch <launch-text>: run as a step body in that session
  reset_stub
  RUN_ENV=("${LEGIT_ENV[@]}")
  run_in "${BUNDLE_PATH}" "${BASH_BIN}" -c 'set -uo pipefail; ARGS=(); '"$1"'
RC=$?; exit "$RC"'; local rc=$?
  RUN_ENV=()
  return "$rc"
}
if [ -n "${LAUNCH}" ]; then
  legit_launch "${LAUNCH}"; rc=$?
  if [ "$rc" -eq 0 ] && grep -qxF "${MERGE_LINE}" "${GH_LOG}" && ! grep -qF 'REFUSED' "${ERR}" \
     && [ "$(pr_state)" = "MERGED" ]; then
    pass "SEC5-G6-legit-session-merges" "step-03's launch clears exported functions, SHELLOPTS and LD_LIBRARY_PATH; the gate merges"
  else
    fail "SEC5-G6-legit-session-merges" "rc=${rc}, state=$(pr_state), stub log: $(tr '\n' '|' < "${GH_LOG}"), stderr: $(head -c 300 "${ERR}")"
  fi
  legit_launch '/bin/bash -p "$GATE_PHYS" "${ARGS[@]}"'; rc=$?
  if [ "$rc" -eq 3 ] && grep -qF "${REFUSED_PREFIX}${L_PR}${REFUSED_SUFFIX}" "${ERR}" && [ "$(pr_state)" = "OPEN" ]; then
    pass "SEC5-G6-legit-session-bare-refused" "the same session launching the gate without step-03's clearing is refused"
  else
    fail "SEC5-G6-legit-session-bare-refused" "rc=${rc} (want 3), state=$(pr_state), stderr: $(head -c 300 "${ERR}")"
  fi
else
  fail "SEC5-G6-legit-session-merges" "could not find step-03's '( unset -f … \"\${ARGS[@]}\" )' launch block"
fi

# SEC-6. Alias names are compared literally, never as patterns.
RUN_ENV=("STUB_ALIASES=m.*: pr merge
p[m]: pr merge
*: pr merge
pa: api -X PUT repos/o/r/pulls/38/merge
")
# A word that matches an alias name only as a pattern is no alias: it is
# refused as an extension, never expanded to the alias's `pr merge`.
refused SEC6-dot-star-not-pattern "$L_EXT" mx 38
refused SEC6-bracket-not-pattern "$L_EXT" pm 38
refused SEC6-star-not-pattern "$L_EXT" zz 38
refused SEC6-literal-dot-star "$L_PR" 'm.*' 38
refused SEC6-literal-bracket "$L_PR" 'p[m]' 38
refused SEC6-alias-to-api "$L_API" pa
RUN_ENV=()

# SEC-7. A refusal is a fixed line: no argv, no secret, nothing on stdout.
# G9: the line ends by telling the agent not to work around the refusal.
FIXED_TAIL=" is only allowed inside autodrive_merge_gate.sh; leave the PR open — auto-drive's merge gate merges after Crusty CLEAN and the merge-ready loop. Do not retry with another gh path, PATH, alias, API client or token."
secret_kept() { # secret_kept <id> <label> <gh-args...>
  local id="$1" label="$2"; shift 2
  reset_stub; agent_gh "$@"; local rc=$?
  if [ "$rc" -eq 3 ] && [ ! -s "${OUT}" ] && ! grep -qF 'SECRET_1551' "${ERR}" \
     && [ "$(cat "${ERR}")" = "${REFUSED_PREFIX}${label}${FIXED_TAIL}" ]; then
    pass "$id" "the refusal is exactly the fixed '${label}' line, with no secret"
  else
    fail "$id" "rc=${rc}, stdout bytes=$(wc -c < "${OUT}"), stderr: $(head -c 300 "${ERR}" | sed 's/SECRET_1551[A-Za-z0-9_]*/<SECRET>/g')"
  fi
}
secret_kept SEC7-pr-body "$L_PR" pr merge 38 --body 'SECRET_1551_body'
secret_kept SEC7-repo "$L_PR" -R SECRET_1551_owner/r pr merge 38
secret_kept SEC7-api-header "$L_API" api -H 'Authorization: token SECRET_1551_tok' -X PUT repos/o/r/pulls/38/merge
secret_kept SEC7-gql "$L_GQL" api graphql -f 'query=mutation{mergePullRequest(input:{pullRequestId:"SECRET_1551_id"}){clientMutationId}}'
secret_kept SEC7-gql-unread "$L_GQL_UNREAD" api graphql -F 'query=@SECRET_1551_file'
secret_kept SEC7-alias "$L_ALIAS" alias set pm 'pr merge SECRET_1551_x'

# SEC8. macOS ships /bin/bash 3.2, where `${x,,}` is a bad substitution that
# ends the script: lowercasing lives in to_lower only, and its tr fallback
# (forced here in a copy) classifies like bash 4 and still passes through.
if [ "$(grep -v '^[[:space:]]*#' "${SHIM}" | grep -c ',,}')" = "1" ] \
   && grep -A3 '^to_lower()' "${SHIM}" | grep -qF 'LOWER="${1,,}"'; then
  pass "SEC8-lower-once" "the shim lowercases with \${x,,} only inside to_lower"
else
  fail "SEC8-lower-once" "\${x,,} outside to_lower breaks the shim under bash 3.2"
fi
OLD_GUARD="${WORK}/old-bash/gh-merge-guard"; mkdir -p "${OLD_GUARD}"
sed 's/BASH_VERSINFO\[0\] >= 4/BASH_VERSINFO[0] >= 99/' "${SHIM}" > "${OLD_GUARD}/gh"
chmod 0755 "${OLD_GUARD}/gh"
grep -qF 'BASH_VERSINFO[0] >= 99' "${OLD_GUARD}/gh" || harness_error "SEC8: could not force the tr fallback"
old_bash_gh() { run_in "${OLD_GUARD}:${STUB}:${TOOLBIN}" "${BASH_BIN}" -c 'gh "$@"' agent-shell "$@"; }
for spec in "3|api -X PUT REPOS/O/R/PULLS/38/MERGE" "3|api graphql -f query=mutation{MergePullRequest(x)}" \
            "3|alias set pm pr_MERGE" "0|api repos/o/r/pulls/38" "0|api graphql -f query={viewer{login}}"; do
  want="${spec%%|*}"; read -r -a argv <<< "${spec#*|}"
  reset_stub; old_bash_gh "${argv[@]}"; rc=$?
  if [ "$rc" = "$want" ] && ! grep -qF 'bad substitution' "${ERR}"; then
    pass "SEC8-tr-fallback" "gh ${argv[*]} exits ${want} with the tr fallback"
  else
    fail "SEC8-tr-fallback" "gh ${argv[*]}: rc=${rc} (want ${want}), stderr: $(head -c 300 "${ERR}")"
  fi
done

# ---------------------------------------------------------------------------
# T6. The advisory directive.
# ---------------------------------------------------------------------------
echo "--- T6: directive ---"
if [ "${#DIRECTIVE}" -lt 400 ]; then
  pass "T6-length" "the directive is under 400 bytes (#1570 budget)"
else
  fail "T6-length" "the directive is ${#DIRECTIVE} bytes"
fi
# step_block <recipe-file> <step-id>: the step's YAML, up to the next step.
step_block() {
  awk -v id="$2" '
    index($0, "- id: \"" id "\"") == 3 { f=1; print; next }
    f && /^  - id: / { exit }
    f { print }' "$1"
}
# step_command <recipe-file> <step-id>: the step's `command: |` body.
step_command() {
  step_block "$1" "$2" | awk '
    /^    command: \|/ { c=1; next }
    c { if ($0 ~ /^    [a-zA-Z_]+:/) exit; sub(/^      /, ""); print }'
}
BUILD="${RECIPES}/autodrive-build.yaml"
S01B="$(step_command "${BUILD}" step-01b-no-merge-directive)"
S01B_BLOCK="$(step_block "${BUILD}" step-01b-no-merge-directive)"
S02_BLOCK="$(step_block "${BUILD}" step-02-default-workflow)"
if [ -n "${S01B}" ] && printf '%s' "${S01B_BLOCK}" | grep -qE '^    type: "?bash"?' \
   && printf '%s' "${S01B_BLOCK}" | grep -qE '^    parse_json: true' \
   && printf '%s' "${S01B_BLOCK}" | grep -qE '^    output: "?no_merge_directive"?'; then
  pass "T6-step01b-shape" "autodrive-build has a bash step-01b with parse_json and output no_merge_directive"
else
  fail "T6-step01b-shape" "autodrive-build.yaml has no bash step-01b-no-merge-directive (parse_json, output no_merge_directive)"
fi
# step-01b sits before step-02.
if grep -nF 'id: "step-01b-no-merge-directive"' "${BUILD}" >/dev/null \
   && [ "$(grep -nF 'id: "step-01b-no-merge-directive"' "${BUILD}" | cut -d: -f1)" -lt "$(grep -nF 'id: "step-02-default-workflow"' "${BUILD}" | cut -d: -f1)" ]; then
  pass "T6-step01b-order" "step-01b runs before step-02"
else
  fail "T6-step01b-order" "step-01b is missing or after step-02"
fi
if printf '%s' "${S02_BLOCK}" | grep -qF 'task_description: "{{no_merge_directive.task_description}}"' \
   && printf '%s' "${S02_BLOCK}" | grep -qF 'requirements: "{{no_merge_directive.requirements}}"' \
   && ! printf '%s' "${S02_BLOCK}" | grep -qF '"{{task_description}}"'; then
  pass "T6-step02-context" "step-02 passes the directive-bearing copies to default-workflow"
else
  fail "T6-step02-context" "step-02 does not template {{no_merge_directive.task_description}} and {{no_merge_directive.requirements}}"
fi
if [ -n "${S01B}" ] && ! printf '%s' "${S01B}" | grep -qF '{{'; then
  pass "T6-step01b-no-template" "step-01b's command templates nothing (values come from the environment)"
else
  fail "T6-step01b-no-template" "step-01b's command contains a {{template}}"
fi

# run_01b <task> <requirements> <no_merge>: step-01b's body with the values
# in RECIPE_VAR_* and the upper-case names, as the runner exports them.
run_01b() {
  rm -f "${WORK}/01b.out" "${WORK}/01b.err"
  mkdir -p "${WORK}/01b-tmp"
  env -i PATH="${RUN01B_PATH:-${TOOLBIN}}" HOME="${WORK}/home" TMPDIR="${WORK}/01b-tmp" LC_ALL=C \
    AMPLIHACK_HOME="${REPO_ROOT}" REPO_PATH="${WORK}" \
    RECIPE_VAR_task_description="$1" TASK_DESCRIPTION="$1" \
    RECIPE_VAR_requirements="$2" REQUIREMENTS="$2" \
    RECIPE_VAR_no_merge="$3" NO_MERGE="$3" \
    "${BASH_BIN}" -c "${S01B}" > "${WORK}/01b.out" 2> "${WORK}/01b.err"
}
# field_01b <name>: a field of step-01b's JSON, byte-exact.
field_01b() {
  local v
  v="$(jq -j --arg f "$1" '.[$f]' < "${WORK}/01b.out" 2>/dev/null; printf x)"
  printf '%s' "${v%x}"
}
count_directive() { # count_directive <text>
  local text="$1" n=0
  while [[ "$text" == *"${DIRECTIVE}"* ]]; do n=$((n + 1)); text="${text#*"${DIRECTIVE}"}"; done
  printf '%s' "$n"
}
# appended_once <id> <input> <output>: output = input + whitespace + directive.
appended_once() {
  local id="$1" in="$2" got="$3" rest
  rest="${got#"$in"}"
  rest="${rest#"${rest%%[![:space:]]*}"}"; rest="${rest%"${rest##*[![:space:]]}"}"
  if [[ "$got" == "$in"* ]] && [ "$rest" = "${DIRECTIVE}" ] && [ "$(count_directive "$got")" = "1" ]; then
    pass "$id" "the directive is appended once, after the original text"
  else
    fail "$id" "want the input followed by the directive once; got: ${got:0:400}"
  fi
}
if [ -n "${S01B}" ]; then
  TD='Fix issue 42: tighten the parser.'
  RQ='R1. Parser rejects empty input.'
  run_01b "$TD" "$RQ" true; rc=$?
  if [ "$rc" -eq 0 ] && jq -e 'type == "object"' < "${WORK}/01b.out" >/dev/null 2>&1; then
    pass "T6-01b-json" "step-01b prints one JSON object"
  else
    fail "T6-01b-json" "rc=${rc}, stdout: $(head -c 300 "${WORK}/01b.out"), stderr: $(head -c 300 "${WORK}/01b.err")"
  fi
  GOT_TD="$(field_01b task_description)"; GOT_RQ="$(field_01b requirements)"
  appended_once T6-01b-task "$TD" "$GOT_TD"
  appended_once T6-01b-req "$RQ" "$GOT_RQ"

  # Idempotent: the step's own output fed back in comes out unchanged.
  run_01b "$GOT_TD" "$GOT_RQ" true
  if [ "$(field_01b task_description)" = "$GOT_TD" ] && [ "$(field_01b requirements)" = "$GOT_RQ" ]; then
    pass "T6-01b-idempotent" "text that already ends with the directive is not given a second copy"
  else
    local_td="$(field_01b task_description)"
    fail "T6-01b-idempotent" "got: ${local_td:0:400}"
  fi

  for nm in "" false FALSE no; do
    run_01b "$TD" "$RQ" "$nm"
    if [ "$(field_01b task_description)" = "$TD" ] && [ "$(field_01b requirements)" = "$RQ" ]; then
      pass "T6-01b-off-${nm:-empty}" "no_merge='${nm}' leaves both texts unchanged"
    else
      local_td="$(field_01b task_description)"
      fail "T6-01b-off-${nm:-empty}" "no_merge='${nm}' changed the text: ${local_td:0:300}"
    fi
  done

  # The bare marker in the input is not the directive: it is still appended.
  TD_M="Earlier note ${MARKER} quoted from an issue."
  run_01b "$TD_M" "$RQ" true
  appended_once T6-01b-marker-only "$TD_M" "$(field_01b task_description)"

  # Quotes, a newline, command substitution and backslashes arrive verbatim
  # and run nothing.
  TD_I=$'line "one" with \'quotes\'\nsecond $(touch '"${WORK}"$'/pwned) `touch '"${WORK}"$'/pwned2` \\ end'
  run_01b "$TD_I" "$RQ" true
  appended_once T6-01b-verbatim "$TD_I" "$(field_01b task_description)"
  if [ ! -e "${WORK}/pwned" ] && [ ! -e "${WORK}/pwned2" ]; then
    pass "T6-01b-no-exec" "text in task_description is never executed"
  else
    fail "T6-01b-no-exec" "step-01b executed text from task_description"
  fi

  # Empty inputs still get the directive.
  run_01b "" "" true
  appended_once T6-01b-empty "" "$(field_01b task_description)"
else
  fail "T6-01b-runtime" "step-01b-no-merge-directive has no command body to run"
fi

# G1. step-01b fails closed: with no_merge=true, a jq that is missing, fails,
# prints nothing, or prints JSON without the directive ends the step non-zero,
# so step-02 never starts with a silently missing directive. With no_merge
# off, the inputs pass through unchanged.
NOJQ="${WORK}/nojq"; mkdir -p "${NOJQ}"
for f in "${TOOLBIN}"/*; do [ "${f##*/}" = jq ] || ln -s "$(readlink "$f" || printf '%s' "$f")" "${NOJQ}/${f##*/}"; done
JQ_STUBS="${WORK}/jq-stubs"
for kind in fail empty nodirective; do
  mkdir -p "${JQ_STUBS}/${kind}"
  case "$kind" in
    fail)        body='exit 1' ;;
    empty)       body='exit 0' ;;
    nodirective) body='printf '"'"'{"task_description":"t","requirements":"r"}\n'"'"'; exit 0' ;;
  esac
  printf '#!%s\n%s\n' "${BASH_BIN}" "${body}" > "${JQ_STUBS}/${kind}/jq"
  chmod 0755 "${JQ_STUBS}/${kind}/jq"
done
# closed_01b <id> <why> <path>: no_merge=true under <path> exits non-zero.
closed_01b() {
  local id="$1" why="$2"
  RUN01B_PATH="$3" run_01b 'Fix issue 42.' 'R1.' true; local rc=$?
  if [ "$rc" -ne 0 ]; then
    pass "$id" "$why: step-01b exits ${rc}"
  else
    fail "$id" "$why: step-01b exited 0 (stdout: $(head -c 200 "${WORK}/01b.out"))"
  fi
}
if [ -n "${S01B}" ]; then
  closed_01b T6-G1-no-jq "jq missing with no_merge=true" "${NOJQ}"
  closed_01b T6-G1-jq-fails "jq exiting 1 with no_merge=true" "${JQ_STUBS}/fail:${TOOLBIN}"
  closed_01b T6-G1-jq-empty "jq printing nothing with no_merge=true" "${JQ_STUBS}/empty:${TOOLBIN}"
  closed_01b T6-G1-no-directive "output without the directive with no_merge=true" "${JQ_STUBS}/nodirective:${TOOLBIN}"
  if grep -qF "#1551" "${WORK}/01b.err"; then
    pass "T6-G1-message" "the fail-closed exit names #1551 on stderr"
  else
    fail "T6-G1-message" "the fail-closed stderr does not name #1551: $(head -c 200 "${WORK}/01b.err")"
  fi
  # no_merge off: unchanged, byte for byte, including odd bytes.
  TD_P=$'keep "this"\n$(x) \\ as is'
  run_01b "${TD_P}" 'R1. as is' false; rc=$?
  if [ "$rc" -eq 0 ] && [ "$(field_01b task_description)" = "${TD_P}" ] \
     && [ "$(field_01b requirements)" = 'R1. as is' ]; then
    pass "T6-G1-off-passthrough" "no_merge=false passes both inputs through unchanged"
  else
    local_td="$(field_01b task_description)"
    fail "T6-G1-off-passthrough" "rc=${rc}, task_description: ${local_td:0:200}"
  fi
else
  fail "T6-G1" "step-01b-no-merge-directive has no command body to run"
fi

# The prompt lines (advisory; the shim is the control).
check_block() { # check_block <id> <recipe> <step> <fixed-string> <why>
  if step_block "${RECIPES}/$2.yaml" "$3" | grep -qF -- "$4"; then
    pass "$1" "$5"
  else
    fail "$1" "$5 -- '$4' not found in $2 $3"
  fi
}
check_block T6-crusty-02 autodrive-crusty-round step-02-crusty-review "${DIRECTIVE}" \
  "the crusty review prompt carries the directive"
check_block T6-crusty-04 autodrive-crusty-round step-04-address-concerns "${DIRECTIVE}" \
  "the crusty fix prompt carries the directive"
check_block T6-merge-round-04 autodrive-merge-round step-04-address-blockers "${DIRECTIVE}" \
  "the merge-round fix prompt carries the directive"
MR04="$(step_block "${RECIPES}/autodrive-merge-round.yaml" step-04-address-blockers)"
if printf '%s\n' "${MR04}" | grep -qxF '      **Round:** {{autodrive_round_label}} — **PR #{{platform_facts.pr}}**' \
   && printf '%s\n' "${MR04}" | grep -qxF '      **Blockers:** {{merge_ready_verdict.blocker_count}}'; then
  pass "T6-G3-no-reflow" "merge-round step-04 keeps main's Round and Blockers header lines"
else
  fail "T6-G3-no-reflow" "merge-round step-04 reflowed the Round/Blockers header (keep main's two lines; #1516 owns that prompt)"
fi
if printf '%s\n' "${MR04}" | grep -A1 -xF "      ${DIRECTIVE}" | sed -n 2p | grep -qxE '[[:space:]]*'; then
  pass "T6-G3-blank-after" "the directive line is followed by a blank line"
else
  fail "T6-G3-blank-after" "the directive line in merge-round step-04 is not its own paragraph"
fi
if [ "$(printf '%s\n' "${MR04}" | grep -cF "${MARKER}")" -eq 1 ]; then
  pass "T6-G3-once" "merge-round step-04 carries the directive exactly once"
else
  fail "T6-G3-once" "merge-round step-04 carries the marker $(printf '%s\n' "${MR04}" | grep -cF "${MARKER}") times"
fi
check_block T6-prep-02 workflow-prep step-02-clarify-requirements "no_merge" \
  "workflow-prep step-02 conditions on no_merge"
check_block T6-prep-02-ready workflow-prep step-02-clarify-requirements "merge-ready" \
  "workflow-prep step-02 ends the requirements at merge-ready when no_merge is true"
check_block T6-finalize-22 workflow-finalize step-22-ensure-mergeable "no_merge" \
  "workflow-finalize step-22 conditions on no_merge"
if step_block "${RECIPES}/workflow-finalize.yaml" step-22-ensure-mergeable | grep -qiF 'do not merge; stop at merge-ready'; then
  pass "T6-finalize-22-line" "workflow-finalize step-22 says: do not merge; stop at merge-ready"
else
  fail "T6-finalize-22-line" "workflow-finalize step-22 lacks 'do not merge; stop at merge-ready'"
fi
if awk '/^context:/{c=1;next} c&&/^[^ #]/{exit} c' "${RECIPES}/default-workflow.yaml" | grep -qE '^  no_merge: ""'; then
  pass "T6-default-workflow-context" "default-workflow defaults no_merge to \"\""
else
  fail "T6-default-workflow-context" "default-workflow.yaml context has no 'no_merge: \"\"'"
fi
# The directive never tells an agent to start an orchestration (#1333).
if ! printf '%s' "${DIRECTIVE}" | grep -qiE 'recipe run|dev-orchestrator|smart-orchestrator|/dev|ultrathink'; then
  pass "T6-no-orchestration" "the directive starts no orchestration"
else
  fail "T6-no-orchestration" "the directive mentions an orchestration"
fi

# ---------------------------------------------------------------------------
# D. Detection (G4, G7, G8): a merge that slipped past the guard (absolute-path
# gh, curl with a token) is never reported as progress. After phase 1 and
# after phase 2 the PR state is re-read; MERGED ends the run with exit 1, and
# a state that cannot be read is read once more, then ends it with exit 1.
# ---------------------------------------------------------------------------
echo "--- D: merged-outside-the-gate detection ---"
# A stub `amplihack` with just the two `orch helper` calls the steps make.
AMPSTUB="${WORK}/ampstub"; mkdir -p "${AMPSTUB}"
{
  printf '#!%s\n' "${BASH_BIN}"
  cat <<'AMP'
[ "${1:-} ${2:-}" = "orch helper" ] || { echo "ampstub: unsupported: $*" >&2; exit 2; }
sub="${3:-}"; shift 3
field=""; def=""
while [ $# -gt 0 ]; do
  case "$1" in
    --require-field|--field) field="${2:-}"; shift 2 ;;
    --default) def="${2:-}"; shift 2 ;;
    *) shift ;;
  esac
done
in="$(cat)"
case "$sub" in
  extract-json) printf '%s' "$in" | jq -e --arg f "$field" 'has($f)' >/dev/null 2>&1 || exit 1; printf '%s\n' "$in" ;;
  extract-field)
    out="$(printf '%s' "$in" | jq -r --arg f "$field" --arg d "$def" '(.[$f] // $d) | tostring' 2>/dev/null)" || out="$def"
    printf '%s\n' "$out" ;;
  *) echo "ampstub: unsupported: $sub" >&2; exit 2 ;;
esac
AMP
} > "${AMPSTUB}/amplihack"
chmod 0755 "${AMPSTUB}/amplihack"
DETECT_PATH="${AMPSTUB}:${STUB}:${TOOLBIN}"
REPO_D="${WORK}/repo-d"; mkdir -p "${REPO_D}"
MERGED_MSG='#1551 merged outside autodrive_merge_gate.sh'

# run_cmd <command-text>: a recipe bash step's body, the way the runner runs
# it, with values only in the environment (RUN_ENV).
run_cmd() {
  local cmd="$1"
  ( cd "${REPO_D}" && run_in "${DETECT_PATH}" "${BASH_BIN}" -c "$cmd" )
}
state_reads() { grep -cE '^pr view 38( .*)? --json [a-zA-Z,]*state' "${GH_LOG}" || true; }

# --- G4/G8: autodrive-build step-03, after phase 1 ---
S03="$(step_command "${BUILD}" step-03-resolve-pr)"
if [ -n "${S03}" ] && ! printf '%s' "${S03}" | grep -qF '{{'; then
  pass "D-G4-no-template" "autodrive-build step-03 templates nothing (values come from the environment)"
else
  fail "D-G4-no-template" "autodrive-build step-03 is missing or contains a {{template}}"
fi
# phase1 <id-suffix> <should_run> <no_merge> <preflight-pr> <stub-state-mode>
preflight_json() { printf '{"should_run":"%s","pr":"%s","pr_url":"","pr_state":"","branch":"feat/i1551","state_dir":"","reason":""}' "$1" "$2"; }
phase1() {
  local sr="$1" nm="$2" pr="$3"
  local pf; pf="$(preflight_json "$sr" "$pr")"
  RUN_ENV=("AMPLIHACK_HOME=${REPO_ROOT}" "REPO_PATH=${REPO_D}"
           "BUILD_PREFLIGHT=${pf}" "RECIPE_VAR_build_preflight=${pf}"
           "NO_MERGE=${nm}" "RECIPE_VAR_no_merge=${nm}"
           "STUB_PR_LIST=38" ${D_ENV[@]+"${D_ENV[@]}"})
  run_cmd "${S03}"; local rc=$?
  RUN_ENV=()
  return "$rc"
}
D_ENV=()

reset_stub; printf 'MERGED' > "${GH_STATE}"
phase1 true true 38; rc=$?
if [ "$rc" -eq 1 ] && grep -qF "${MERGED_MSG}" "${ERR}"; then
  pass "D-G4-merged" "a PR found MERGED after phase 1 ends the run with exit 1 and names #1551"
else
  fail "D-G4-merged" "rc=${rc} (want 1), stderr: $(head -c 400 "${ERR}")"
fi
if ! grep -qF '"pr":"38"' "${OUT}"; then
  pass "D-G4-merged-no-result" "a merged-outside-the-gate phase 1 prints no build_result"
else
  fail "D-G4-merged-no-result" "phase 1 still reported a build_result: $(head -c 200 "${OUT}")"
fi
# Fresh build: no PR in the preflight, found by branch, then found MERGED.
reset_stub; printf 'MERGED' > "${GH_STATE}"
phase1 true true ""; rc=$?
if [ "$rc" -eq 1 ] && grep -qF "${MERGED_MSG}" "${ERR}"; then
  pass "D-G4-merged-fresh" "a fresh phase 1 whose PR (found by branch) is MERGED exits 1"
else
  fail "D-G4-merged-fresh" "rc=${rc} (want 1), stderr: $(head -c 400 "${ERR}")"
fi
reset_stub
phase1 true true 38; rc=$?
if [ "$rc" -eq 0 ] && jq -e '.pr == "38"' < "${OUT}" >/dev/null 2>&1 && [ "$(state_reads)" -ge 1 ]; then
  pass "D-G4-open" "an OPEN PR after phase 1 passes, and its state was read"
else
  fail "D-G4-open" "rc=${rc} (want 0), state reads=$(state_reads), stdout: $(head -c 200 "${OUT}"), stderr: $(head -c 300 "${ERR}")"
fi
# The check belongs to a run that built (should_run) under no_merge.
reset_stub; printf 'MERGED' > "${GH_STATE}"
phase1 true false 38; rc=$?
if [ "$rc" -eq 0 ] && ! grep -qF "${MERGED_MSG}" "${ERR}"; then
  pass "D-G4-no-merge-off" "with no_merge off phase 1 does not treat MERGED as a #1551 violation"
else
  fail "D-G4-no-merge-off" "rc=${rc} (want 0), stderr: $(head -c 300 "${ERR}")"
fi
reset_stub; printf 'MERGED' > "${GH_STATE}"
phase1 false true 38; rc=$?
if [ "$rc" -eq 0 ] && ! grep -qF "${MERGED_MSG}" "${ERR}"; then
  pass "D-G4-skipped-phase" "a resumed run that skipped phase 1 (should_run=false) is not checked here"
else
  fail "D-G4-skipped-phase" "rc=${rc} (want 0), stderr: $(head -c 300 "${ERR}")"
fi
# G8: unreadable state: read twice, then exit 1.
reset_stub
D_ENV=("STUB_MODE=unreadable")
phase1 true true 38; rc=$?
D_ENV=()
if [ "$rc" -eq 1 ] && [ "$(state_reads)" -eq 2 ] && grep -qF '#1551' "${ERR}"; then
  pass "D-G8-unreadable-phase1" "an unreadable PR state after phase 1 is retried once, then exits 1"
else
  fail "D-G8-unreadable-phase1" "rc=${rc} (want 1), state reads=$(state_reads) (want 2), stderr: $(head -c 300 "${ERR}")"
fi

# S4: REPO_PATH is the PR's own tree. A helper planted there that always
# passes is never sourced: without one under AMPLIHACK_HOME the check exits 1.
PLANT_HOME="${WORK}/plant-home"; mkdir -p "${PLANT_HOME}"
mkdir -p "${REPO_D}/amplifier-bundle/tools"
cat > "${REPO_D}/amplifier-bundle/tools/autodrive_state.sh" <<PLANT
autodrive_require_unmerged() { : > "${WORK}/planted-sourced"; return 0; }
autodrive_mark_phase_done() { :; }
PLANT
# planted <id> <phase-fn> <AMPLIHACK_HOME value>: exit 1, the planted helper unused.
planted() {
  local id="$1" fn="$2"; rm -f "${WORK}/planted-sourced"; reset_stub; printf 'MERGED' > "${GH_STATE}"
  D_ENV=("AMPLIHACK_HOME=$3")
  "$fn" true true 38; local rc=$?
  D_ENV=()
  if [ "$rc" -eq 1 ] && [ ! -e "${WORK}/planted-sourced" ] && grep -qF 'not found under AMPLIHACK_HOME' "${ERR}"; then
    pass "$id" "a helper planted in REPO_PATH is not sourced when AMPLIHACK_HOME (${3:-<empty>}) lacks it; exit 1"
  else
    fail "$id" "rc=${rc} (want 1), planted sourced: $([ -e "${WORK}/planted-sourced" ] && echo yes || echo no), stderr: $(head -c 300 "${ERR}")"
  fi
}
planted D-S4-planted-phase1 phase1 "${PLANT_HOME}"
planted D-S4-planted-phase1-empty-home phase1 ""
# cd fails closed: PR state is never read from whatever repo cwd belongs to.
reset_stub
D_ENV=("REPO_PATH=${WORK}/no-such-repo")
phase1 true true 38; rc=$?
D_ENV=()
if [ "$rc" -eq 1 ] && grep -qF 'cannot enter REPO_PATH' "${ERR}" && ! grep -q '^pr view' "${GH_LOG}"; then
  pass "D-cd-phase1" "an unenterable REPO_PATH exits 1 before any PR state is read"
else
  fail "D-cd-phase1" "rc=${rc} (want 1), stub log: $(tr '\n' '|' < "${GH_LOG}"), stderr: $(head -c 300 "${ERR}")"
fi

# --- G7/G8: auto-drive-to-merge, after phase 2 and before phase 3 ---
ADTM="${RECIPES}/auto-drive-to-merge.yaml"
ADTM_IDS="$(grep -oE '^  - id: "[^"]+"' "${ADTM}" | sed -E 's/^  - id: "([^"]+)"/\1/')"
P2_ID="$(printf '%s\n' "${ADTM_IDS}" | awk 'p { print; exit } $0 == "autodrive-crusty-loop" { p=1 }')"
if [ -n "${P2_ID}" ] && [ "${P2_ID}" != "autodrive-merge-loop" ] \
   && step_block "${ADTM}" "${P2_ID}" | grep -qE '^    type: "?bash"?'; then
  pass "D-G7-step" "auto-drive-to-merge runs a bash check (${P2_ID}) between the crusty loop and the merge loop"
else
  fail "D-G7-step" "the step after autodrive-crusty-loop is '${P2_ID:-none}', not a bash check before autodrive-merge-loop"
fi
P2="$( [ -n "${P2_ID}" ] && step_command "${ADTM}" "${P2_ID}" || true)"
if [ -n "${P2}" ] && ! printf '%s' "${P2}" | grep -qF '{{'; then
  pass "D-G7-no-template" "the phase-2 check templates nothing"
else
  fail "D-G7-no-template" "the phase-2 check is missing or contains a {{template}}"
fi
# phase2 <pr>: the check with the build phase's result in the environment.
phase2() {
  local br; br="$(printf '{"pr":"%s","pr_url":"","state_dir":""}' "$1")"
  RUN_ENV=("AMPLIHACK_HOME=${REPO_ROOT}" "REPO_PATH=${REPO_D}"
           "BUILD_RESULT=${br}" "RECIPE_VAR_build_result=${br}"
           "PR_NUMBER=$1" "RECIPE_VAR_pr_number=$1"
           ${D_ENV[@]+"${D_ENV[@]}"})
  run_cmd "${P2}"; local rc=$?
  RUN_ENV=()
  return "$rc"
}
if [ -n "${P2}" ]; then
  reset_stub; printf 'MERGED' > "${GH_STATE}"
  phase2 38; rc=$?
  if [ "$rc" -eq 1 ] && grep -qF "${MERGED_MSG}" "${ERR}"; then
    pass "D-G7-merged" "a PR found MERGED after phase 2 ends the run before phase 3 with exit 1"
  else
    fail "D-G7-merged" "rc=${rc} (want 1), stderr: $(head -c 400 "${ERR}")"
  fi
  reset_stub
  phase2 38; rc=$?
  if [ "$rc" -eq 0 ] && [ "$(state_reads)" -ge 1 ] && ! grep -qF 'REFUSED' "${ERR}"; then
    pass "D-G7-open" "an OPEN PR after phase 2 continues to phase 3"
  else
    fail "D-G7-open" "rc=${rc} (want 0), state reads=$(state_reads), stderr: $(head -c 300 "${ERR}")"
  fi
  reset_stub
  D_ENV=("STUB_MODE=unreadable")
  phase2 38; rc=$?
  D_ENV=()
  if [ "$rc" -eq 1 ] && [ "$(state_reads)" -eq 2 ]; then
    pass "D-G8-unreadable-phase2" "an unreadable PR state after phase 2 is retried once, then exits 1"
  else
    fail "D-G8-unreadable-phase2" "rc=${rc} (want 1), state reads=$(state_reads) (want 2), stderr: $(head -c 300 "${ERR}")"
  fi
  # S4 and the fail-closed cd, as for phase 1. planted() calls
  # `<fn> true true 38`; phase2 takes only the PR number.
  phase2_as1() { phase2 "$3"; }
  planted D-S4-planted-phase2 phase2_as1 "${PLANT_HOME}"
  planted D-S4-planted-phase2-relative-home phase2_as1 "relative/home"
  reset_stub
  D_ENV=("REPO_PATH=${WORK}/no-such-repo")
  phase2 38; rc=$?
  D_ENV=()
  if [ "$rc" -eq 1 ] && grep -qF 'cannot enter REPO_PATH' "${ERR}" && ! grep -q '^pr view' "${GH_LOG}"; then
    pass "D-cd-phase2" "an unenterable REPO_PATH exits 1 before any PR state is read"
  else
    fail "D-cd-phase2" "rc=${rc} (want 1), stub log: $(tr '\n' '|' < "${GH_LOG}"), stderr: $(head -c 300 "${ERR}")"
  fi
  # G8: the PR number must be digits; anything else is never passed to gh.
  for bad in '38x' '38;touch '"${WORK}"'/pwned-pr' '' '-1' '$(id)'; do
    rm -f "${WORK}/pwned-pr"; reset_stub
    phase2 "$bad"; rc=$?
    if [ "$rc" -ne 0 ] && ! grep -q '^pr view' "${GH_LOG}" && [ ! -e "${WORK}/pwned-pr" ]; then
      pass "D-G8-digits-'${bad}'" "a PR number of '${bad}' exits non-zero and reaches no gh call"
    else
      fail "D-G8-digits-'${bad}'" "rc=${rc} (want non-zero), stub log: $(tr '\n' '|' < "${GH_LOG}")"
    fi
  done
else
  fail "D-G7-runtime" "no phase-2 check command to run"
fi

echo ""
echo "═══════════════════════════════"
echo "Results: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
echo "═══════════════════════════════"
[ "${FAIL_COUNT}" -eq 0 ]
