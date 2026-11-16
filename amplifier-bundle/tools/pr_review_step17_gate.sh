#!/usr/bin/env bash
# pr_review_step17_gate.sh — the body of `step-17f-verification-gate` in amplifier-bundle/recipes/workflow-pr-review.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: the step runs it with
# `bash`, so it reads the same environment the recipe runner exported and
# writes the same stdout the step's `output:` captures. It is a file rather
# than an inline block so the recipe stays under its line budget; compressing
# the recipe instead is the one thing the brick rule forbids.

echo "=== STEP 17 VERIFICATION GATE ===" && \
echo "" && \
echo "Before proceeding, verify ALL of the following:" && \
echo "" && \
echo "  ✓ Did I invoke the REVIEWER agent? → Yes (output captured)" && \
echo "  ✓ Did I invoke the SECURITY agent? → Yes (output captured)" && \
echo "  ✓ Did I invoke the PHILOSOPHY-GUARDIAN agent? → Yes (output captured)" && \
echo "  ✓ Did I check for FORBIDDEN PATTERN violations? → Yes" && \
echo "  ✓ Are all three reviews POSTED to the PR as comments?" && \
echo "  ✓ All blocking issues have been addressed?" && \
echo "" && \
echo "If any verification fails, STOP and complete the missing step." && \
echo "" && \
echo "=== GATE PASSED - Proceed to Step 18 ==="
