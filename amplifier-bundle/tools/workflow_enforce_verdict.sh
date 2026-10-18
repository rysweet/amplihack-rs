#!/usr/bin/env bash
# Sourced by its canonical step after selecting one framework/context snapshot.
WORKFLOW_CONTEXT_COHORT=implementation
IMPL="$(workflow_context_read implementation || :)"
VERDICT_RAW="$(workflow_context_read verdict_json || :)"
workflow_context_capture SELECTED_ALLOW_NO_OP allow_no_op

# Fast-path opt-outs (issue #425 parity).
_ORCH_SENTINEL='No files modified — orchestration task'
if [ -n "$IMPL" ] && printf '%s' "$IMPL" | grep -qF -- "$_ORCH_SENTINEL"; then
  echo "INFO: step-08c-enforce-verdict orchestration opt-out — sentinel matched (issue #425)." >&2
  echo "  Skipping work-verifier verdict for orchestration / docs-only task." >&2
  exit 0
fi
if [ "$SELECTED_ALLOW_NO_OP" = "true" ]; then
  echo "INFO: step-08c-enforce-verdict orchestration opt-out — ALLOW_NO_OP=true (issue #425)." >&2
  echo "  Skipping work-verifier verdict; classification permits no working-tree edits." >&2
  exit 0
fi

# ----------------------------------------------------------------------
# Parse the verifier's JSON verdict (last JSON object on stdout).
# ----------------------------------------------------------------------
if [ -z "$VERDICT_RAW" ]; then
  echo "WARN: step-08c-work-verifier produced no output. Treating as INSUFFICIENT_EVIDENCE per fail-safe contract (issue #615)." >&2
  echo "  Letting workflow continue with a loud warning rather than masking a possible real failure." >&2
  exit 0
fi

# Issue #1062 (finding A1): route the verifier's JSON verdict through the
# tested `orch helper` toolchain instead of grep/awk/jq/case text-scraping.
#   extract-json      -> first complete JSON object ({} if none parseable)
#   extract-field     -> .verdict, or INSUFFICIENT_EVIDENCE if absent
#   normalise-verdict -> collapse LLM synonyms via exact-token equality
#                        (preserves the #624 synonym mapping and the #615
#                        fail-safe default; UNVERIFIED/NOT_APPROVED never
#                        collide with a pass token).
# A non-parseable blob yields VERDICT_LINE="{}" and
# VERDICT="INSUFFICIENT_EVIDENCE", which the gate below degrades safely
# (loud WARNING + exit 0), preserving the prior behaviour.
VERDICT_LINE=$(printf '%s' "$VERDICT_RAW" | amplihack orch helper extract-json)
VERDICT=$(printf '%s' "$VERDICT_LINE" \
  | amplihack orch helper extract-field --field verdict --default INSUFFICIENT_EVIDENCE \
  | amplihack orch helper normalise-verdict)

case "$VERDICT" in
  WORK_VERIFIED)
    echo "INFO: step-08c work-verifier APPROVED (issue #615)." >&2
    printf '%s\n' "$VERDICT_LINE" | jq . >&2 || printf '%s\n' "$VERDICT_LINE" >&2
    exit 0
    ;;
  INSUFFICIENT_EVIDENCE)
    echo "WARN: step-08c work-verifier returned INSUFFICIENT_EVIDENCE — fail-safe: letting workflow continue with a loud warning (issue #615/#624). An unknown/novel verdict normalises here too, so a recipe with real artifacts is never hard-failed." >&2
    printf '%s\n' "$VERDICT_LINE" | jq . >&2 || printf '%s\n' "$VERDICT_LINE" >&2
    exit 0
    ;;
  HOLLOW_SUCCESS)
    echo "ERROR: step-08c work-verifier rejected step-08-implement with HOLLOW_SUCCESS (issue #615)." >&2
    echo "  The implement step claimed COMPLETE but the verifier could not find concrete artifacts that address the task." >&2
    printf '%s\n' "$VERDICT_LINE" | jq . >&2 || printf '%s\n' "$VERDICT_LINE" >&2
    exit 1
    ;;
  *)
    # Defensive fallback (issue #624/#1062): `normalise-verdict` already
    # collapses unknown/novel verdict strings to INSUFFICIENT_EVIDENCE
    # upstream, so this branch is normally unreachable. It stays as a
    # belt-and-suspenders fail-safe: an unexpected token must never
    # hard-fail a recipe that already has real artifacts (e.g. an open PR).
    echo "WARN: step-08c unexpected verdict '$VERDICT' — fail-safe to INSUFFICIENT_EVIDENCE (issue #624)." >&2
    printf '%s\n' "$VERDICT_LINE" | jq . >&2 || printf '%s\n' "$VERDICT_LINE" >&2
    exit 0
    ;;
esac
