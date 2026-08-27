#!/usr/bin/env bash
# workflow_plan_banner.sh — the 23-step plan `step-00-workflow-preparation`
# prints when a default-workflow run starts.
#
# This is the banner verbatim, moved out of `amplifier-bundle/recipes/
# workflow-prep.yaml`. It was extracted rather than trimmed: that brick sits at
# the 400-line budget enforced by `every_phase_subrecipe_under_400_lines`, and
# the rule's remedy for a full brick is extraction to `amplifier-bundle/tools/`,
# not compressing the file until the counter is satisfied (see the header of
# scripts/check-brick-budget.sh). The room bought here pays for the identity
# preflight added alongside it in step-00a (issue #1290).
#
# Pure output: no arguments, no environment, no side effects.

set -euo pipefail

echo "=== WORKFLOW PREPARATION COMPLETE ==="
echo ""
echo "23-Step Workflow Initialized (with granular sub-steps for Steps 17-19):"
echo "  Step 0: Workflow Preparation - CURRENT"
echo "  Step 1: Prepare Workspace"
echo "  Step 2: Rewrite/Clarify Requirements"
echo "  Step 3: Create GitHub Issue"
echo "  Step 4: Setup Worktree/Branch"
echo "  Step 5: Research and Design"
echo "  Step 6: Retcon Documentation"
echo "  Step 7: TDD - Write Tests First"
echo "  Step 8: Implement Solution"
echo "  Step 9: Refactor and Simplify"
echo "  Step 10: Review Before Commit"
echo "  Step 11: Incorporate Feedback"
echo "  Step 12: Run Pre-commit Hooks"
echo "  Step 13: Mandatory Local Testing"
echo "  Step 14: Bump Version (MANDATORY)"
echo "  Step 15: Commit and Push"
echo "  Step 16: Open PR as Draft"
echo "  Step 17: Review the PR (MANDATORY - 6 sub-steps 17a-17f with verification gate)"
echo "  Step 18: Implement Review Feedback (MANDATORY - 5 sub-steps 18a-18e with verification gate)"
echo "  Step 19: Philosophy Compliance Check (4 sub-steps 19a-19d with verification gate)"
echo "  Step 20: Final Cleanup"
echo "  Step 20c: Quality Audit Loop (3+ cycles, multi-agent validation)"
echo "  Step 21: Convert PR to Ready"
echo "  Step 22: Ensure PR Mergeable"
echo ""
