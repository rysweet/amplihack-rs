#!/usr/bin/env bash
# autodrive_merge_method.sh — section 7b of autodrive_merge_gate.sh (#1888):
# which of --merge, --squash and --rebase the gate's fixed merge argv uses.
#
# Only the gate sources this file, from its own physical directory, so it runs
# inside the process the gh merge guard trusts to merge; it is a gate file
# (GATE_FILES in merge_guard.rs). It DEFINES one function and runs nothing.
#
#   autodrive_merge_method <merge_method-context>
#
# reads the gate's STATE_JSON, HEAD_SHA, BASE_SHA (section 6d's fetch, when it
# ran), PWD (the checkout), AMPLIHACK_BIN and its field/note/block functions.
# On success it sets MERGE_FLAG and records five evidence lines; otherwise it
# records one blocker carrying the selector's reason and leaves MERGE_FLAG
# empty. Returns 0, 1, or 79 for a policy refusal from `gh api`.
#
# The choice itself is `amplihack orch helper select-merge-method` (Rust,
# crates/amplihack-cli/src/commands/merge_method/): the explicit context;
# else .amplihack/auto-drive.toml at the base branch tip fetched now (the
# base's policy at merge time), never the working tree, refusing a pull
# request whose own changes touch it; else the first of squash, merge and
# rebase that is allowed. Allowed means the repository settings allow it AND
# every ruleset pull_request rule on the base branch (baseRefName) that lists
# allowed_merge_methods lists it AND, for merge, no ruleset
# required_linear_history rule is active on the base branch (GitHub then
# accepts only squash or rebase). The repository is the one the pull request
# URL names. A disallowed method is never used and nothing falls back.
# Contract: docs/reference/auto-drive-to-merge.md, "Choosing the merge method".

# shellcheck disable=SC2034 # MERGE_FLAG belongs to the gate, which reads it after this returns
autodrive_merge_method() {
  local explicit="${1:-}" base_ref="" base="" out="" rc=0 reason=""
  MERGE_FLAG=""
  # The rulesets are read on the base branch whatever the source.
  base_ref="$(field "$STATE_JSON" baseRefName "")"
  # The file is read only without an explicit method, at the fetched base tip.
  if [ -z "$explicit" ]; then
    base="${BASE_SHA:-}"
    if [ -z "$base" ] && declare -F autodrive_base_sha >/dev/null; then
      base="$(autodrive_base_sha "$PWD" "$base_ref")" || base=""
    fi
  fi
  out="$("$AMPLIHACK_BIN" orch helper select-merge-method --pr-url "$(field "$STATE_JSON" url "")" \
    --repo-root "$PWD" --base-ref "$base_ref" --base "$base" --head "${HEAD_SHA:-}" --merge-method "$explicit")"
  rc=$?
  if [ "$rc" -ne 0 ]; then
    # The selector prints {"error": reason}: kept to one printable line.
    reason="$(printf '%s' "$out" | jq -er 'if type == "object" and (.error | type) == "string" then .error else empty end' 2>/dev/null \
      | tr -c '[:print:]' ' ' | cut -c1-400)"
    block "merge-method selection failed (exit ${rc}): ${reason:-the helper gave no reason}"
    [ "$rc" -eq 79 ] && return 79
    return 1
  fi
  # Exactly one object, every field in its closed set; anything else, prose
  # around it included, is a version mismatch or a broken helper.
  if ! printf '%s' "$out" | jq -e -s 'length == 1 and (.[0] | type == "object"
      and (.method == "merge" or .method == "squash" or .method == "rebase")
      and (.source == "explicit" or .source == "repository" or .source == "default")
      and (.repository | type == "string" and test("^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$"))
      and (.config_commit == null or (.config_commit | type == "string" and test("^([0-9a-f]{40}|[0-9a-f]{64})$")))
      and ((.source == "explicit") == (.config_commit == null))
      and (.rationale | type == "string" and length > 0))' >/dev/null 2>&1; then
    block "merge-method helper returned malformed or incompatible output"
    return 1
  fi
  note "merge_method=$(printf '%s' "$out" | jq -r .method)"
  note "merge_method_source=$(printf '%s' "$out" | jq -r .source)"
  note "merge_method_repository=$(printf '%s' "$out" | jq -r .repository)"
  note "merge_method_config_commit=$(printf '%s' "$out" | jq -r '.config_commit // "not-read"')"
  # JSON string encoding keeps the rationale on one evidence line.
  note "merge_method_rationale=$(printf '%s' "$out" | jq -c .rationale)"
  case "$(printf '%s' "$out" | jq -r .method)" in
    merge) MERGE_FLAG="--merge" ;;
    squash) MERGE_FLAG="--squash" ;;
    rebase) MERGE_FLAG="--rebase" ;;
  esac
  return 0
}
