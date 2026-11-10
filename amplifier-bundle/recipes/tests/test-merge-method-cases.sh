# Sourced by test-auto-drive-to-merge.sh after gate_run and trusted fixtures.
# No standalone harness: retain real head, QA, review, private-state and guard checks.
mm_permissions() {
  GH_METHODS_JSON="$(jq -cn --argjson m "$1" --argjson s "$2" --argjson r "$3" \
    '{allow_merge_commit:$m,allow_squash_merge:$s,allow_rebase_merge:$r}')"
  export GH_METHODS_JSON
}
mm_case() { # label expected-method source [explicit-method]
  local label="$1" method="$2" source="$3" rc bundle
  shift 3
  local -a args=()
  [ $# -eq 0 ] || args=(--merge-method "$1")
  GATE_PRIV=1 gate_run merges --round-record "$REC" --qa-evidence "$QA" ${args[@]+"${args[@]}"}; rc=$?
  if [ -z "$method" ]; then
    if [ "$rc" -ne 0 ] && ! grep -q '^pr merge ' "${GATE_DIR}/gh-calls" \
       && printf '%s' "$GATE_OUT" | grep -qF '"merge_result":"NOT_MERGED"'; then
      pass "1888-${label}" "rejected selection records blockers and executes no merge"
    else
      fail "1888-${label}" "rejected selection reached merge or lacked blocker result: rc=$rc $GATE_OUT"
    fi
    return
  fi
  bundle="$(printf '%s' "$GATE_OUT" | jq -r '.evidence_bundle // empty' 2>/dev/null)"
  if [ "$rc" -eq 0 ] && printf '%s' "$GATE_OUT" | grep -qF '"merge_result":"MERGED"' \
     && [ "$(grep -c '^pr merge ' "${GATE_DIR}/gh-calls")" -eq 1 ] \
     && grep -qxF "pr merge 42 --repo owner-a/repo-a --${method} --delete-branch --match-head-commit ${GATE_HEAD}" "${GATE_DIR}/gh-calls" \
     && [ -f "$bundle" ] && grep -qF "head_sha=${GATE_HEAD}" "$bundle" \
     && grep -qF "merge_method=${method}" "$bundle" \
     && grep -qF "merge_method_source=${source}" "$bundle" \
     && grep -qE 'merge_method_rationale=.+ ' "$bundle"; then
    pass "1888-${label}" "one allowed literal flag, exact head and method evidence, confirmed MERGED"
  else
    fail "1888-${label}" "selection/argv/evidence/confirmation failed: rc=$rc $GATE_OUT $(cat "${GATE_DIR}/err")"
  fi
}
# Every case uses the isolated fixture checkout, never the developer repository.
mkdir -p "${FX}/.github"
mm_permissions true false false; mm_case merge-only merge default
mm_permissions false true false; mm_case squash-only squash default
mm_permissions false false true; mm_case rebase-only rebase default
mm_permissions true true true; mm_case default-order squash default
mm_permissions true false true; mm_case fallback-order merge default
mm_permissions false false false; mm_case none-allowed '' ''
mm_permissions true true true
printf 'merge_method: merge\n' > "${FX}/.github/amplihack.yml"
mm_case repository merge repository
mm_case empty-context merge repository ''
mm_case explicit rebase explicit rebase
mm_permissions false true false
mm_case disallowed-repository '' ''
mm_case allowed-override squash explicit squash
mm_case disallowed-explicit '' '' rebase
printf 'merge_method: [\n' > "${FX}/.github/amplihack.yml"
mm_case invalid-config '' ''
mm_case invalid-config-explicit '' '' squash
rm -f "${FX}/.github/amplihack.yml"
mm_permissions true true true
mm_case invalid-context '' '' auto
GH_METHODS_RC=79; export GH_METHODS_RC
mm_case api-failure '' ''
unset GH_METHODS_RC
GH_METHODS_JSON='[true,false,false]'; export GH_METHODS_JSON
mm_case api-positional-array '' ''
for GH_METHODS_JSON in '{}' 'null' 'bad-json' \
  '{"allow_merge_commit":true,"allow_squash_merge":false}' \
  '{"allow_merge_commit":true,"allow_squash_merge":"true","allow_rebase_merge":false}'; do
  export GH_METHODS_JSON
  mm_case api-schema '' ''
done
unset GH_METHODS_JSON
# Version skew or a broken helper must not be accepted by extract-json's
# permissive prose extraction. Delegate all existing helper calls unchanged.
MM_HELPER_DIR="${WORK_PHYS}/method-helper"; mkdir -p "$MM_HELPER_DIR"
cat > "$MM_HELPER_DIR/amplihack" <<'MM_HELPER'
#!/bin/bash
if [ "${1:-} ${2:-} ${3:-}" = 'orch helper select-merge-method' ]; then
  cat "$(dirname "$0")/response"
  exit 0
fi
exec "$REAL_AMPLIHACK" "$@"
MM_HELPER
chmod 0755 "$MM_HELPER_DIR/amplihack"
for mm_bad in \
  '' '{}' 'null' '[]' \
  '{"method":"merge","source":"default","rationale":null}' \
  '{"method":"merge","source":"unknown","rationale":"invalid source"}' \
  '{"method":"--admin","source":"explicit","rationale":"invalid method"}' \
  '{"method":"merge","source":"default","rationale":"one"}{"method":"squash","source":"default","rationale":"two"}' \
  'prefix {"method":"merge","source":"default","rationale":"prose"}' \
  '{"method":"merge","source":"default","rationale":"trailing"} garbage'; do
  printf '%s\n' "$mm_bad" > "$MM_HELPER_DIR/response"
  GATE_HELPER="$MM_HELPER_DIR/amplihack" mm_case malformed-helper '' ''
done
# An executable lacking the new subcommand is a fail-closed version mismatch.
cat > "$MM_HELPER_DIR/missing" <<'MM_MISSING'
#!/bin/bash
if [ "${1:-} ${2:-} ${3:-}" = 'orch helper select-merge-method' ]; then
  echo 'selector unavailable' >&2
  exit 2
fi
exec "$REAL_AMPLIHACK" "$@"
MM_MISSING
chmod 0755 "$MM_HELPER_DIR/missing"
GATE_HELPER="$MM_HELPER_DIR/missing" mm_case missing-helper '' ''

# Conflicting ambient identity must never influence any gate operation.
GH_REPO=owner-b/repo-b GATE_EXTRA_ENV=GH_REQUIRE_TARGET=owner-a/repo-a mm_case explicit-repository squash default
if grep -q '^repo view' "${GATE_DIR}/gh-calls" || [ -f "${GATE_DIR}/gh-calls.wrong-target" ]; then
  fail "1888-repository-binding" "a PR operation used ambient identity"
elif grep -q '^api repos/owner-a/repo-a$' "${GATE_DIR}/gh-calls"; then
  pass "1888-repository-binding" "permission query and every PR operation use explicit target"
else
  fail "1888-repository-binding" "permission query missing"
fi
for missing_identity in "" owner "/repo" "owner/repo/extra"; do
  GATE_REPOSITORY="$missing_identity" gate_run merges --round-record "$REC" --qa-evidence "$QA"; rc=$?
  if [ "$rc" -ne 0 ] && [ ! -s "${GATE_DIR}/gh-calls" ]; then
    pass "1888-invalid-identity" "invalid or missing target fails before gh"
  else
    fail "1888-invalid-identity" "invalid identity reached gh"
  fi
done
