#!/usr/bin/env bash
# smart_routing_gap_report.sh — the `file-routing-bug` step of
# smart-execute-routing.yaml (adaptive error recovery, fix #3132).
#
# Files a GitHub issue saying no smart-orchestrator execution path ran, and
# prints its URL. When gh is unavailable or fails, saves the same report to a
# temp file and prints `diag:<file>`. Never fails the run.
#
# This is the step body moved out of the recipe, which sits at the 400-line
# brick budget (see the header of scripts/check-brick-budget.sh). The room it
# frees pays for the context keys declared in #1684.
#
# Reads this run's own step outputs as RECIPE_VAR_<key> only (#1561, #1684):
# task_type, workstream_count, force_single_workstream, recursion_guard and
# adaptive_recipe. A bare TASK_TYPE or ADAPTIVE_RECIPE left by a parent run is
# ignored. TASK_DESCRIPTION is the caller's own input.

TASK_TYPE="${RECIPE_VAR_task_type:-}"
WS_COUNT="${RECIPE_VAR_workstream_count:-}"
FORCE_SINGLE="${RECIPE_VAR_force_single_workstream:-}"
GUARD="${RECIPE_VAR_recursion_guard:-}"
RECIPE="${RECIPE_VAR_adaptive_recipe:-}"
# FIX (#469/#311): Use env var — safe against injection and word-splitting.
TASK_DESC="${TASK_DESCRIPTION:-}"

# Build diagnostic body
BODY=$(cat <<EOFBODY
## smart-orchestrator routing gap: no execution path ran

**Symptom**: All execution routing conditions evaluated to false.
No execution step (single, parallel, or blocked-fallback) ran.

### Diagnostic context
- task_type: \`$TASK_TYPE\`
- workstream_count: \`$WS_COUNT\`
- force_single_workstream: \`$FORCE_SINGLE\`
- recursion_guard: \`$GUARD\`
- adaptive_recipe selected: \`$RECIPE\`

### Task description (truncated)
$(echo "$TASK_DESC" | head -c 500)

### Environment
- AMPLIHACK_HOME: \`${AMPLIHACK_HOME:-<unset>}\`
- recipe-runner-rs: \`$(which recipe-runner-rs 2>/dev/null || echo '<not found>')\`
- hostname: \`$(hostname -s 2>/dev/null || echo unknown)\`
- date: \`$(date -u +%Y-%m-%dT%H:%M:%SZ)\`

### Expected behavior
At least one execution path should have run based on the classification.
The adaptive strategy step ran \`$RECIPE\` directly to recover.

---
*Auto-filed by smart-orchestrator adaptive error recovery — fix #3132*
EOFBODY
)

# Attempt to file a GH issue — non-fatal if gh is unavailable or auth fails
ISSUE_URL=$(gh issue create \
  --title "smart-orchestrator routing gap: $TASK_TYPE task, workstream_count=$WS_COUNT" \
  --body "$BODY" \
  --label "bug" 2>/dev/null) || true

if [ -n "$ISSUE_URL" ]; then
  echo "[ADAPTIVE] Filed infrastructure bug: $ISSUE_URL" >&2
  echo "$ISSUE_URL"
else
  # Log the diagnostic info so it's preserved even if filing fails
  DIAG_FILE=$(mktemp /tmp/smart-orch-diag-XXXXXX.md)
  echo "$BODY" > "$DIAG_FILE"
  echo "[ADAPTIVE] Could not file GH issue (gh unavailable or auth failed)." >&2
  echo "[ADAPTIVE] Diagnostic info saved to: $DIAG_FILE" >&2
  echo "diag:$DIAG_FILE"
fi
