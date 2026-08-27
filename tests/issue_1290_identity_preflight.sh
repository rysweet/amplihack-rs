#!/usr/bin/env bash
# Issue #1290 — the acting GitHub identity must be verified before any step
# does real work, and the verification must not become a gate of its own.
#
# The incident: a run completed workflow preparation, workspace preparation,
# requirements clarification, codebase analysis, ambiguity resolution and host
# detection, then failed at `step-03-create-issue` because the account that
# happened to be active was not authorised for the target repository. Six steps
# of work discarded over a condition knowable before the first one started.
#
# These tests drive the real helper and the real recipe step against real git
# repositories and a fake `gh`. Three properties are under test:
#
#   1. an authorised account passes, in exactly ONE API call;
#   2. an unauthorised account fails with all three facts in the message —
#      which account is active, which repository was targeted, how to switch;
#   3. a check that cannot be made warns and continues — no network, no `gh`,
#      no helper, a rate limit or an API error must never stop a run (#1268).
#
# Every token in this file is an obvious fake.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HELPER="$ROOT/amplifier-bundle/tools/workflow_identity_preflight.sh"
RECIPE="$ROOT/amplifier-bundle/recipes/workflow-prep.yaml"

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

[ -f "$HELPER" ] || { echo "missing $HELPER"; exit 1; }

WORK=$(mktemp -d "${TMPDIR:-/tmp}/issue1290.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# Keep git from reading the caller's identity, hooks or ambient repository.
export GIT_CONFIG_NOSYSTEM=1
export HOME="$WORK/fakehome"; mkdir -p "$HOME"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@e
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@e
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
unset GH_TOKEN GITHUB_TOKEN GH_HOST AMPLIHACK_SKIP_IDENTITY_PREFLIGHT

# --- a fake `gh` -----------------------------------------------------------
# Modes are chosen by FAKE_GH; every call is appended to $GH_CALL_LOG so the
# "one API call, once" claim can be checked rather than asserted.
BIN="$WORK/bin"; mkdir -p "$BIN"
cat > "$BIN/gh" <<'FAKEEOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${GH_CALL_LOG:-/dev/null}"
if [ "${1:-}" = "auth" ]; then
  # Deliberately includes a token-shaped string so redaction is exercised.
  cat <<'AUTH'
github.com
  x Failed to log in to github.com account managed-account (keyring)
  - Active account: true
  - Token: ghp_FAKEFAKEFAKEFAKEFAKEFAKEFAKE0001
  ✓ Logged in to github.com account personal-account (keyring)
  - Active account: false
AUTH
  exit 0
fi
case "${FAKE_GH:-ok_write}" in
  ok_write)   printf '%s\n' '{"data":{"viewer":{"login":"personal-account"},"repository":{"nameWithOwner":"example-org/example-repo","viewerPermission":"WRITE"}}}' ;;
  ok_admin)   printf '%s\n' '{"data":{"viewer":{"login":"personal-account"},"repository":{"nameWithOwner":"example-org/example-repo","viewerPermission":"ADMIN"}}}' ;;
  read_only)  printf '%s\n' '{"data":{"viewer":{"login":"managed-account"},"repository":{"nameWithOwner":"example-org/example-repo","viewerPermission":"READ"}}}' ;;
  no_perm)    printf '%s\n' '{"data":{"viewer":{"login":"personal-account"},"repository":null}}' ;;
  emu)        echo 'GraphQL: Unauthorized: As an Enterprise Managed User, you cannot access this content (repository)' >&2; exit 1 ;;
  policy)     echo 'Error: Access denied by policy settings' >&2; exit 1 ;;
  not_found)  echo "GraphQL: Could not resolve to a Repository with the name 'example-org/example-repo'. (repository)" >&2; exit 1 ;;
  bad_creds)  echo 'gh: Bad credentials (HTTP 401)' >&2; exit 1 ;;
  network)    echo 'error connecting to api.github.com: dial tcp: lookup api.github.com: no such host' >&2; exit 1 ;;
  ratelimit)  echo 'gh: API rate limit exceeded for user ID 4242. (HTTP 403)' >&2; exit 1 ;;
  server)     echo 'gh: HTTP 502 Bad Gateway' >&2; exit 1 ;;
  weird)      echo 'gh: something nobody has ever seen before' >&2; exit 1 ;;
  slow)       sleep 5 ;;
esac
FAKEEOF
chmod +x "$BIN/gh"

mkrepo() { # mkrepo <dir> <origin-url|->
  git init -q -b main "$1"
  [ "$2" = "-" ] || git -C "$1" remote add origin "$2"
}
mkrepo "$WORK/gh-https" "https://github.com/example-org/example-repo.git"
mkrepo "$WORK/gh-ssh"   "git@github.com:example-org/example-repo.git"
mkrepo "$WORK/azdo"     "https://dev.azure.com/example-org/example-project/_git/example-repo"
mkrepo "$WORK/noremote" "-"
mkdir -p "$WORK/notagit"

# run <repo> [env assignments...] -> sets OUT / RC
run() {
  local repo="$1"; shift
  GH_CALL_LOG="$WORK/calls.log"; : > "$GH_CALL_LOG"
  OUT="$(env PATH="$BIN:$PATH" GH_CALL_LOG="$GH_CALL_LOG" "$@" bash "$HELPER" "$repo" 2>&1)"
  RC=$?
}

echo "== 1. an authorised account passes, in one API call =="
run "$WORK/gh-https" FAKE_GH=ok_write
if [ "$RC" -eq 0 ] && grep -q 'identity_preflight: ok' <<<"$OUT"; then
  pass "authorised account (WRITE) passes"
else
  fail "authorised account rejected: rc=$RC out=${OUT:0:200}"
fi
grep -q 'personal-account' <<<"$OUT" && grep -q 'example-org/example-repo' <<<"$OUT" \
  && pass "the passing run still names the acting account and the repository" \
  || fail "passing run does not name account and repository: ${OUT:0:200}"
graphql_calls=$(grep -c 'api graphql' "$WORK/calls.log")
total_calls=$(wc -l < "$WORK/calls.log")
if [ "$graphql_calls" -eq 1 ] && [ "$total_calls" -eq 1 ]; then
  pass "exactly one API call on the happy path"
else
  fail "expected 1 gh call, saw $total_calls: $(tr '\n' '|' < "$WORK/calls.log")"
fi
run "$WORK/gh-ssh" FAKE_GH=ok_admin
[ "$RC" -eq 0 ] && grep -q 'permission=ADMIN' <<<"$OUT" \
  && pass "an scp-form origin (git@github.com:owner/repo.git) is parsed and passes" \
  || fail "scp-form origin mishandled: rc=$RC out=${OUT:0:200}"

echo "== 2. an unauthorised account fails with all three facts =="
three_facts() { # three_facts <label> <expected-account>
  local label="$1" acct="$2" missing=""
  grep -q "$acct" <<<"$OUT" || missing="$missing account"
  grep -q 'example-org/example-repo' <<<"$OUT" || missing="$missing repository"
  grep -q 'gh auth switch' <<<"$OUT" || missing="$missing switch-instructions"
  if [ "$RC" -eq 1 ] && [ -z "$missing" ]; then
    pass "$label fails (exit 1) naming account, repository and how to switch"
  else
    fail "$label: rc=$RC missing:${missing:-none} out=${OUT:0:300}"
  fi
}
run "$WORK/gh-https" FAKE_GH=read_only;  three_facts "read-only permission" "managed-account"
run "$WORK/gh-https" FAKE_GH=emu;        three_facts "a managed-identity refusal" "managed-account"
run "$WORK/gh-https" FAKE_GH=policy;     three_facts "an org policy denial (#1279/#1280 shape)" "managed-account"
run "$WORK/gh-https" FAKE_GH=not_found;  three_facts "a repository the account cannot resolve" "managed-account"
run "$WORK/gh-https" FAKE_GH=bad_creds;  three_facts "an expired or invalid token" "managed-account"

# The general condition, not one vendor's phrasing of it.
# Comments in the helper name both phrases, precisely to say they are NOT what
# it matches; the executable lines must not mention either.
HELPER_CODE="$(grep -v '^[[:space:]]*#' "$HELPER")"
if grep -qi 'Enterprise Managed User' <<<"$HELPER_CODE" || grep -qi 'Access denied by policy' <<<"$HELPER_CODE"; then
  fail "the helper special-cases a vendor phrase; the condition is 'this identity cannot act here'"
else
  pass "the helper matches the general condition, not 'EMU' or 'policy denial'"
fi

echo "== 3. a stray environment token is called out, never printed =="
FAKE_TOKEN="ghp_FAKEFAKEFAKEFAKEFAKEFAKEFAKE0002"
run "$WORK/gh-https" FAKE_GH=read_only GH_TOKEN="$FAKE_TOKEN"
if [ "$RC" -eq 1 ] && grep -q 'GH_TOKEN' <<<"$OUT"; then
  pass "an exported GH_TOKEN is named as the thing overriding gh auth switch"
else
  fail "GH_TOKEN not mentioned: rc=$RC out=${OUT:0:300}"
fi
if grep -q "$FAKE_TOKEN" <<<"$OUT" || grep -q 'ghp_FAKEFAKEFAKEFAKEFAKEFAKEFAKE0001' <<<"$OUT"; then
  fail "a token value reached the output"
else
  pass "no token value reaches the output (both fakes redacted)"
fi

echo "== 4. a check that cannot be made warns and continues =="
continues() { # continues <label> <expected-substring>
  if [ "$RC" -eq 0 ] && grep -q 'WARNING' <<<"$OUT" && grep -q "$2" <<<"$OUT"; then
    pass "$1 warns and continues (exit 0)"
  else
    fail "$1: rc=$RC out=${OUT:0:300}"
  fi
}
run "$WORK/gh-https" FAKE_GH=network;   continues "a network failure" "network-or-server-error"
run "$WORK/gh-https" FAKE_GH=server;    continues "a 502 from the API" "network-or-server-error"
run "$WORK/gh-https" FAKE_GH=ratelimit; continues "a rate limit" "rate-limited"
run "$WORK/gh-https" FAKE_GH=weird;     continues "an unrecognised API error" "unrecognised-api-error"
run "$WORK/gh-https" FAKE_GH=no_perm;   continues "a 200 that reports no permission" "permission-not-reported"
run "$WORK/gh-https" FAKE_GH=slow AMPLIHACK_IDENTITY_PREFLIGHT_TIMEOUT=1
continues "an API call that times out" "timed-out"

# `gh` absent entirely: an empty PATH containing only the coreutils it needs.
STUB="$WORK/stub"; mkdir -p "$STUB"
for c in git sed awk grep head sort tr mktemp rm timeout printf env bash cat wc; do
  p="$(command -v "$c" 2>/dev/null)" && ln -sf "$p" "$STUB/$c"
done
OUT="$(env -i PATH="$STUB" HOME="$HOME" GIT_CONFIG_NOSYSTEM=1 bash "$HELPER" "$WORK/gh-https" 2>&1)"; RC=$?
continues "gh not installed" "gh-not-installed"

echo "== 5. nothing to check is not a failure either =="
run "$WORK/azdo" FAKE_GH=ok_write
if [ "$RC" -eq 0 ] && grep -q 'non-github-remote' <<<"$OUT" && [ ! -s "$WORK/calls.log" ]; then
  pass "an Azure DevOps remote is skipped without calling gh at all"
else
  fail "azdo remote mishandled: rc=$RC calls=$(wc -l < "$WORK/calls.log") out=${OUT:0:200}"
fi
run "$WORK/noremote" FAKE_GH=ok_write
[ "$RC" -eq 0 ] && grep -q 'no-origin-remote' <<<"$OUT" \
  && pass "a repository with no origin is skipped" \
  || fail "no-origin repo mishandled: rc=$RC out=${OUT:0:200}"
run "$WORK/notagit" FAKE_GH=ok_write
[ "$RC" -eq 0 ] && grep -q 'not-a-git-repository' <<<"$OUT" \
  && pass "a non-repository directory is skipped" \
  || fail "non-repo mishandled: rc=$RC out=${OUT:0:200}"
run "$WORK/gh-https" FAKE_GH=read_only AMPLIHACK_SKIP_IDENTITY_PREFLIGHT=1
[ "$RC" -eq 0 ] && grep -q 'skipped' <<<"$OUT" \
  && pass "the documented escape hatch bypasses the check" \
  || fail "escape hatch did not work: rc=$RC out=${OUT:0:200}"

echo "== 6. the recipe step is wired to the helper =="
STEP_CMD="$WORK/step.sh"
if python3 - "$RECIPE" "$STEP_CMD" <<'PY'
import sys, yaml
recipe, out = sys.argv[1], sys.argv[2]
steps = yaml.safe_load(open(recipe))["steps"]
ids = [s["id"] for s in steps]
assert "step-00a-identity-preflight" in ids, ids
assert ids.index("step-00a-identity-preflight") < ids.index("step-02-clarify-requirements")
step = next(s for s in steps if s["id"] == "step-00a-identity-preflight")
open(out, "w").write(step["command"])
PY
then
  pass "workflow-prep declares step-00a-identity-preflight before any real work"
else
  fail "workflow-prep does not declare the preflight step ahead of the working steps"
fi

if [ -s "$STEP_CMD" ]; then
  # The step, run for real, with the helper reachable via AMPLIHACK_HOME.
  : > "$WORK/calls.log"
  OUT="$(cd "$WORK" && env PATH="$BIN:$PATH" GH_CALL_LOG="$WORK/calls.log" FAKE_GH=ok_write \
      AMPLIHACK_HOME="$ROOT" REPO_PATH="$WORK/gh-https" bash "$STEP_CMD" 2>&1)"; RC=$?
  [ "$RC" -eq 0 ] && grep -q 'identity_preflight: ok' <<<"$OUT" \
    && pass "the recipe step resolves the helper via AMPLIHACK_HOME and passes" \
    || fail "recipe step failed with an authorised account: rc=$RC out=${OUT:0:300}"

  : > "$WORK/calls.log"
  OUT="$(cd "$WORK" && env PATH="$BIN:$PATH" GH_CALL_LOG="$WORK/calls.log" FAKE_GH=emu \
      AMPLIHACK_HOME="$ROOT" REPO_PATH="$WORK/gh-https" bash "$STEP_CMD" 2>&1)"; RC=$?
  [ "$RC" -eq 1 ] && grep -q 'gh auth switch' <<<"$OUT" \
    && pass "the recipe step fails the run when the identity is refused" \
    || fail "recipe step did not fail on refusal: rc=$RC out=${OUT:0:300}"

  # Helper unreachable from every cascade location: warn, do not gate. This is
  # the shape of a bundled install whose tools/ directory is incomplete.
  EMPTY="$WORK/empty"; mkdir -p "$EMPTY"
  OUT="$(cd "$EMPTY" && env PATH="$BIN:$PATH" FAKE_GH=ok_write HOME="$WORK/fakehome" \
      AMPLIHACK_HOME="$EMPTY" REPO_PATH="$EMPTY" bash "$STEP_CMD" 2>&1)"; RC=$?
  [ "$RC" -eq 0 ] && grep -q 'WARNING' <<<"$OUT" && grep -q 'helper-not-found' <<<"$OUT" \
    && pass "a missing helper warns and continues rather than gating the run" \
    || fail "missing helper gated the run: rc=$RC out=${OUT:0:300}"
fi

echo
if [ "$fails" -eq 0 ]; then
  echo "issue #1290 identity preflight: all checks passed"
else
  echo "issue #1290 identity preflight: $fails check(s) failed"
fi
exit $((fails > 0))
