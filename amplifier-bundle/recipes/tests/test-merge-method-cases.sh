# #1888 merge-method cases. Sourced by test-auto-drive-to-merge.sh after
# gate_run, the fixture repository FX and REC/QA for GATE_HEAD exist; no
# standalone harness, so every case keeps the real head, QA, review,
# private-state and guard checks of the trusted gate.
#
# The file is read at the base branch tip the gate fetches, so a policy the
# base gains after a pull request branched applies to that pull request too.
# History used here, on top of the main fixture (main is FX_M1; the merge
# base of GATE_HEAD=CR_SHA2 and main is FX_M0):
#
#   FX_M1 (main) --- MM_BASE (base adds .amplihack/auto-drive.toml) ----- refs/heads/mm-main
#                       \        \
#                        \        MM_DROP (base deletes the file again) -- refs/heads/mm-drop
#                         \
#   CR_SHA2 -------------- MM_HEAD (a clean merge of mm-main)
#   CR_SHA2 (not updated: its merge base with mm-main is still FX_M0)
#   CR_SHA2 --- MM_ADD (the pull request adds the file itself)
#   FX_M1 --- MM_BAD (base adds an invalid file) ---- refs/heads/mm-bad; MM_BAD_HEAD merges it

mm_permissions() { # mm_permissions <merge> <squash> <rebase>: what gh api reports
  GH_METHODS_JSON="$(jq -cn --argjson m "$1" --argjson s "$2" --argjson r "$3" \
    '{allow_merge_commit:$m,allow_squash_merge:$s,allow_rebase_merge:$r}')"
  export GH_METHODS_JSON
}
# mm_rules <json>: the active rules gh api reports on the base branch.
mm_rules() { GH_RULES_JSON="$1"; export GH_RULES_JSON; }
mm_pr_rule() { # mm_pr_rule <method>...: one pull_request rule listing them
  jq -cn '[{type:"pull_request",parameters:{required_approving_review_count:0,allowed_merge_methods:$ARGS.positional}}]' --args "$@"
}
mm_bundle() { printf '%s' "$GATE_OUT" | jq -r '.evidence_bundle // empty' 2>/dev/null; }
# mm_files <head>: a MERGE_READY record and PASS qa evidence bound to <head>,
# in MM_REC and MM_QA (gate_files is defined later in the suite).
mm_files() {
  MM_REC="${WORK}/mm-rec-${1}.json"; MM_QA="${WORK}/mm-qa-${1}.json"
  mr_rec MERGE_READY "$1" > "$MM_REC"; qa_fixture "$1" PASS 3 > "$MM_QA"
}

# mm_merged <label> <method> <source> <config-commit|not-read> [gate args...]:
# the gate merges with exactly --<method>, at the exact head, and records why.
mm_merged() {
  local label="$1" method="$2" source="$3" commit="$4" rc bundle head="${GH_HEAD:-$GATE_HEAD}"
  shift 4
  GATE_PRIV=1 gate_run merges --round-record "${MM_REC:-$REC}" --qa-evidence "${MM_QA:-$QA}" "$@"; rc=$?
  bundle="$(mm_bundle)"
  if [ "$rc" -eq 0 ] && printf '%s' "$GATE_OUT" | grep -qF '"merge_result":"MERGED"' \
     && [ "$(grep -c '^pr merge ' "${GATE_DIR}/gh-calls")" -eq 1 ] \
     && grep -qxF "pr merge 42 --${method} --delete-branch --match-head-commit ${head}" "${GATE_DIR}/gh-calls" \
     && [ -f "$bundle" ] && grep -qF "head_sha=${head}" "$bundle" \
     && grep -qxF "  - merge_method=${method}" "$bundle" \
     && grep -qxF "  - merge_method_source=${source}" "$bundle" \
     && grep -qxF "  - merge_method_repository=owner-a/repo-a" "$bundle" \
     && grep -qxF "  - merge_method_config_commit=${commit}" "$bundle" \
     && grep -qE '^  - merge_method_rationale="Selected .+"$' "$bundle"; then
    pass "1888-${label}" "merged with --${method} (${source}, config ${commit}) and the evidence says why"
  else
    fail "1888-${label}" "rc=$rc $GATE_OUT | $(grep -E 'BLOCKER|merge_method' "${GATE_DIR}/err" | tr '\n' ' ') | $(grep '^pr merge' "${GATE_DIR}/gh-calls" 2>/dev/null)"
  fi
}

# mm_refused <label> <reason> [gate args...]: nothing merges, and the
# evidence bundle's BLOCKERS name the reason, not only a generic line.
mm_refused() {
  local label="$1" reason="$2" rc bundle
  shift 2
  GATE_PRIV=1 gate_run merges --round-record "${MM_REC:-$REC}" --qa-evidence "${MM_QA:-$QA}" "$@"; rc=$?
  MM_RC=$rc
  bundle="$(mm_bundle)"
  if [ "$rc" -ne 0 ] && ! grep -q '^pr merge ' "${GATE_DIR}/gh-calls" \
     && printf '%s' "$GATE_OUT" | grep -qF '"merge_result":"NOT_MERGED"' \
     && [ -f "$bundle" ] && sed -n '/^BLOCKERS$/,$p' "$bundle" | grep -qF -- "$reason"; then
    pass "1888-${label}" "refused; the evidence names: ${reason}"
  else
    fail "1888-${label}" "rc=$rc $GATE_OUT | want '${reason}' | $(grep -F 'BLOCKER' "${GATE_DIR}/err" | tr '\n' ' ')"
  fi
}

# --- the default order, from what the repository allows -------------------
# FX has no .amplihack, so the file is looked for at the base tip FX_M1.
mm_permissions true false false; mm_merged merge-only merge default "$FX_M1"
mm_permissions false true false; mm_merged squash-only squash default "$FX_M1"
mm_permissions false false true; mm_merged rebase-only rebase default "$FX_M1"
mm_permissions true true true;   mm_merged default-order squash default "$FX_M1"
mm_permissions true false true;  mm_merged fallback-order merge default "$FX_M1"
mm_permissions false false false
mm_refused none-allowed "merge-method selection failed (exit 1): the repository allows no merge method"

# --- the explicit context ---------------------------------------------------
mm_permissions true true true
mm_merged explicit rebase explicit not-read --merge-method rebase
mm_merged empty-context squash default "$FX_M1" --merge-method ''
mm_permissions false true false
mm_merged allowed-override squash explicit not-read --merge-method squash
mm_refused disallowed-explicit "selected merge method rebase is disallowed by repository permissions" --merge-method rebase
mm_permissions true true true
mm_refused invalid-context "merge_method context must be exactly merge, squash or rebase" --merge-method auto

# --- the working tree is never read -----------------------------------------
mkdir -p "${FX}/.amplihack"
printf 'merge_method = "rebase"\n' > "${FX}/.amplihack/auto-drive.toml"
mm_merged working-tree-ignored squash default "$FX_M1"
rm -rf "${FX:?}/.amplihack"

# --- the file at the base branch tip ----------------------------------------
MM_BASE="$(fx_change "$FX_M1" "base sets the merge method" '.amplihack/auto-drive.toml=merge_method = "merge"')"
MM_HEAD="$(fx_merge "$CR_SHA2" "$MM_BASE")"
MM_DROP="$(fx_change "$MM_BASE" "base drops the merge method" '.amplihack/auto-drive.toml=@delete')"
MM_BAD="$(fx_change "$FX_M1" "base sets an invalid merge method" '.amplihack/auto-drive.toml=merge_method = merge')"
MM_BAD_HEAD="$(fx_merge "$CR_SHA2" "$MM_BAD")"
MM_ADD="$(fx_change "$CR_SHA2" "pull request sets its own merge method" '.amplihack/auto-drive.toml=merge_method = "rebase"')"
if git -C "$FX" push -q origin "${MM_BASE}:refs/heads/mm-main" "${MM_DROP}:refs/heads/mm-drop" \
     "${MM_BAD}:refs/heads/mm-bad" >/dev/null 2>&1; then
  # The in-flight pull request: GATE_HEAD branched at FX_M0, before the base
  # gained the file, and was never brought up to date (an unprotected base
  # never reports BEHIND). The base's policy still decides its merge.
  MM_REC=""; MM_QA=""
  GH_BASE=mm-main mm_merged base-tip-in-flight merge repository "$MM_BASE"
  mm_files "$MM_HEAD"
  GH_BASE=mm-main GH_HEAD="$MM_HEAD" mm_merged repository merge repository "$MM_BASE"
  # A local origin/mm-main that is stale is overwritten by the gate's fetch.
  git -C "$FX" update-ref refs/remotes/origin/mm-main "$FX_M1"
  GH_BASE=mm-main GH_HEAD="$MM_HEAD" mm_merged repository-base-fetched merge repository "$MM_BASE"
  GH_BASE=mm-main GH_HEAD="$MM_HEAD" mm_merged repository-explicit-wins rebase explicit not-read --merge-method rebase
  mm_permissions false true true
  GH_BASE=mm-main GH_HEAD="$MM_HEAD" mm_refused disallowed-repository \
    "selected merge method merge is disallowed by repository permissions; no other method is tried"
  mm_permissions true true true
  # The base dropped the file after this head merged it in: the base tip has
  # no file, so the default applies, whatever the merge base held.
  GH_BASE=mm-drop GH_HEAD="$MM_HEAD" mm_merged base-tip-dropped squash default "$MM_DROP"

  mm_files "$MM_BAD_HEAD"
  GH_BASE=mm-bad GH_HEAD="$MM_BAD_HEAD" mm_refused invalid-file \
    ".amplihack/auto-drive.toml at base tip ${MM_BAD}: syntax"
  # An explicit method means the file is not read, so it cannot block.
  GH_BASE=mm-bad GH_HEAD="$MM_BAD_HEAD" mm_merged invalid-file-explicit squash explicit not-read --merge-method squash

  # The pull request adds the file: refused, as a pull request changing
  # .amplihack/qa.toml is. Crusty reviewed MM_ADD here, so the selection is
  # the only thing that can block.
  mm_files "$MM_ADD"
  CR_SHA="$MM_ADD" GH_HEAD="$MM_ADD" mm_refused changed-in-pr \
    ".amplihack/auto-drive.toml: changed-in-pr"
  CR_SHA="$MM_ADD" GH_HEAD="$MM_ADD" mm_merged changed-in-pr-explicit merge explicit not-read --merge-method merge
  MM_REC=""; MM_QA=""
else
  fail "1888-fixture" "could not push the merge-method base branches to the fixture origin"
fi

# --- an unknown merge base fails closed; an explicit method does not need one
git -C "$FX" remote set-url origin "${WORK_PHYS}/no-such-origin.git"
mm_refused base-unfetched "merge-method selection failed (exit 1): the base branch was not fetched"
mm_merged base-unfetched-explicit merge explicit not-read --merge-method merge
git -C "$FX" remote set-url origin "${FX_ORIGIN}"

# --- the rulesets on the base branch -----------------------------------------
# GitHub's pull_request rule can limit the merge methods without touching the
# repository's allow_* fields; the allowed set is the intersection of both.
mm_permissions true true true
mm_rules "$(mm_pr_rule merge)"
mm_merged ruleset-only-merge merge default "$FX_M1"
mm_refused ruleset-explicit-disallowed \
  "selected merge method squash is disallowed by the rulesets on branch main (pull_request allowed_merge_methods allow merge); no other method is tried" \
  --merge-method squash
mm_permissions false true false
mm_refused ruleset-settings-mismatch \
  "the repository settings allow squash and the rulesets on branch main allow merge: no merge method is allowed by both"
mm_permissions true true true
# Two pages: a reader that stops at the first would allow merge.
mm_rules "$(mm_pr_rule merge rebase)$(mm_pr_rule rebase)"
mm_merged ruleset-second-page rebase default "$FX_M1"
mm_rules '[{"type":"required_linear_history"},{"type":"pull_request","parameters":{"required_approving_review_count":1}}]'
mm_merged ruleset-no-method-limit squash default "$FX_M1"
# The values are literal JSON for the gh stub to print, not shell words.
# shellcheck disable=SC2089,SC2090
for GH_RULES_JSON in '{}' 'null' 'bad-json' '[{"type":"pull_request"}]' \
  '[{"type":"pull_request","parameters":{"allowed_merge_methods":"merge"}}]'; do
  export GH_RULES_JSON
  mm_refused ruleset-schema "invalid answer for the rulesets on branch main"
done
unset GH_RULES_JSON
GH_RULES_RC=3; export GH_RULES_RC
mm_refused ruleset-unreadable "gh api could not read the rulesets on branch main"
GH_RULES_RC=79
mm_refused ruleset-policy-refusal "gh api refused to read the rulesets on branch main (terminal policy exit 79)"
if [ "$MM_RC" -eq 79 ]; then
  pass "1888-ruleset-policy-refusal-exit" "a policy refusal reading the rulesets is surfaced as exit 79"
else
  fail "1888-ruleset-policy-refusal-exit" "exit ${MM_RC}, want 79"
fi
unset GH_RULES_RC

# --- the permission read ----------------------------------------------------
GH_METHODS_RC=79; export GH_METHODS_RC
mm_refused api-policy-refusal "gh api refused to read repository merge permissions (terminal policy exit 79)"
if [ "$MM_RC" -eq 79 ]; then
  pass "1888-api-policy-refusal-exit" "a policy refusal from gh api is surfaced as exit 79"
else
  fail "1888-api-policy-refusal-exit" "exit ${MM_RC}, want 79"
fi
GH_METHODS_RC=3
mm_refused api-failure "gh api could not read repository merge permissions"
unset GH_METHODS_RC
# The values are literal JSON for the gh stub to print, not shell words.
# shellcheck disable=SC2089,SC2090
for GH_METHODS_JSON in '[true,false,false]' '{}' 'null' 'bad-json' \
  '{"allow_merge_commit":true,"allow_squash_merge":false}' \
  '{"allow_merge_commit":true,"allow_squash_merge":"true","allow_rebase_merge":false}'; do
  export GH_METHODS_JSON
  mm_refused api-schema "merge permissions"
done
unset GH_METHODS_JSON

# --- the helper's output contract -------------------------------------------
# Version skew or a broken helper must not be accepted by extract-json's
# permissive prose extraction. Every other helper call is delegated unchanged.
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
MM_OK='{"method":"merge","source":"default","repository":"owner-a/repo-a","config_commit":"'"$FX_M0"'","rationale":"Selected merge"}'
printf '%s\n' "$MM_OK" > "$MM_HELPER_DIR/response"
GATE_HELPER="$MM_HELPER_DIR/amplihack" mm_merged helper-positive-control merge default "$FX_M0"
for mm_bad in \
  '' '{}' 'null' '[]' \
  '{"method":"merge","source":"default","repository":"owner-a/repo-a","config_commit":null,"rationale":"default needs a commit"}' \
  '{"method":"merge","source":"explicit","repository":"owner-a/repo-a","config_commit":"'"$FX_M0"'","rationale":"explicit reads no file"}' \
  '{"method":"merge","source":"default","repository":"owner-a/repo-a","config_commit":"HEAD","rationale":"not a sha"}' \
  '{"method":"merge","source":"default","config_commit":"'"$FX_M0"'","rationale":"no repository"}' \
  '{"method":"merge","source":"default","repository":"owner-a/repo-a","config_commit":"'"$FX_M0"'","rationale":null}' \
  '{"method":"merge","source":"unknown","repository":"owner-a/repo-a","config_commit":"'"$FX_M0"'","rationale":"invalid source"}' \
  '{"method":"--admin","source":"explicit","repository":"owner-a/repo-a","config_commit":null,"rationale":"invalid method"}' \
  "${MM_OK}${MM_OK}" \
  "prefix ${MM_OK}" \
  "${MM_OK} garbage"; do
  printf '%s\n' "$mm_bad" > "$MM_HELPER_DIR/response"
  GATE_HELPER="$MM_HELPER_DIR/amplihack" mm_refused malformed-helper "merge-method helper returned malformed or incompatible output"
done
# An executable lacking the subcommand is a fail-closed version mismatch.
cat > "$MM_HELPER_DIR/missing" <<'MM_MISSING'
#!/bin/bash
if [ "${1:-} ${2:-} ${3:-}" = 'orch helper select-merge-method' ]; then
  echo 'selector unavailable' >&2
  exit 2
fi
exec "$REAL_AMPLIHACK" "$@"
MM_MISSING
chmod 0755 "$MM_HELPER_DIR/missing"
GATE_HELPER="$MM_HELPER_DIR/missing" mm_refused missing-helper "merge-method selection failed (exit 2): the helper gave no reason"

# The repository comes from the pull request URL, never from GH_REPO.
GATE_EXTRA_ENV=GH_REPO=owner-b/repo-b mm_merged gh-repo-ignored squash default "$FX_M1"
if grep -qxF 'api --hostname github.com repos/owner-a/repo-a' "${GATE_DIR}/gh-calls" \
   && grep -qxF 'api --hostname github.com --paginate repos/owner-a/repo-a/rules/branches/main' "${GATE_DIR}/gh-calls"; then
  pass "1888-permissions-from-pr-url" "the settings and ruleset reads name the repository in the pull request URL and its base branch"
else
  fail "1888-permissions-from-pr-url" "$(grep '^api' "${GATE_DIR}/gh-calls" | tr '\n' '|')"
fi
