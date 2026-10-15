#!/usr/bin/env bash
# Sourced by its canonical step after selecting one framework/context snapshot.
# Durable artifact references — each guarded so an absent ref is omitted
# rather than raising an unbound-variable error. Allow-list only; these are
# non-sensitive metadata (no tokens/secrets), with fallbacks for the
# alternate names the runner may flatten depending on the producing step.
BRANCH_REF="${BRANCH_NAME:-${WORKTREE_SETUP_BRANCH:-}}"
PR_NUMBER_REF="${PR_NUMBER:-${PULL_REQUEST_NUMBER:-}}"
PR_URL_REF="${PR_URL:-${PULL_REQUEST_URL:-}}"
COMMIT_REF="${COMMIT_SHA:-${HEAD_SHA:-}}"
REVIEW_THREAD_REF="${REVIEW_THREAD_ID:-${REVIEW_COMMENT_ID:-}}"

# Classify the review outcome. Issue #1062 (finding A3): STATUS comes from
# the doc-review agent's structured `status` field (emitted via parse_json)
# via the tested `orch helper` pipeline — not English-keyword NLU over
# free-text. Untrusted feedback stays DATA: piped through `printf '%s'` into
# the helper, never interpolated into a command (issue #834 S2). Empty or
# non-OK status is treated conservatively as a follow-up item.
DOC_STATUS_RAW=$(printf '%s' "$DOC_FEEDBACK" \
  | amplihack orch helper extract-json \
  | amplihack orch helper extract-field --field status --default "")
DOC_STATUS_NORM=$(printf '%s' "$DOC_STATUS_RAW" | tr '[:lower:]' '[:upper:]' | tr -d '[:space:]')
STATUS="OK"
REASON="documentation-review passed"
if [ -z "$DOC_FEEDBACK" ]; then
  STATUS="NEEDS_ATTENTION"
  REASON="documentation-review produced no feedback (step may have failed)"
elif [ "$DOC_STATUS_NORM" != "OK" ]; then
  STATUS="NEEDS_ATTENTION"
  REASON="documentation-review reported issues requiring follow-up"
fi

if [ "$STATUS" = "NEEDS_ATTENTION" ]; then
  # Visible, non-swallowed diagnostic on stderr.
  echo "WARNING: step-06b-documentation-review did not cleanly pass: ${REASON}." >&2
  echo "WARNING: continuing — documentation review is a quality signal, not a gate;" >&2
  echo "WARNING: durable work (commits/PRs/review threads) is preserved and summarized below." >&2
fi

# Machine-consumable summary on stdout — becomes the step output and lands
# in the recipe summary. Only fixed markers + allow-listed refs are emitted.
echo "DOC_REVIEW_CHECKPOINT: ${STATUS}"
echo "  reason: ${REASON}"
if [ "$STATUS" = "NEEDS_ATTENTION" ]; then
  echo "  marker: NEEDS_ATTENTION"
  echo "  follow_up: re-run documentation-review or address the noted gaps"
fi
echo "  durable_artifacts:"
[ -n "$BRANCH_REF" ]        && echo "    branch: ${BRANCH_REF}"
[ -n "$PR_NUMBER_REF" ]     && echo "    pr_number: ${PR_NUMBER_REF}"
[ -n "$PR_URL_REF" ]        && echo "    pr_url: ${PR_URL_REF}"
[ -n "$COMMIT_REF" ]        && echo "    commit_sha: ${COMMIT_REF}"
[ -n "$REVIEW_THREAD_REF" ] && echo "    review_thread: ${REVIEW_THREAD_REF}"

# Structurally non-fatal: always succeed so reconciliation/summary runs.
exit 0
