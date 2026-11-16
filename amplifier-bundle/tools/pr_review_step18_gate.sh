#!/usr/bin/env bash
# pr_review_step18_gate.sh — the body of `step-18e-verification-gate` in amplifier-bundle/recipes/workflow-pr-review.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: the step runs it with
# `bash`, so it reads the same environment the recipe runner exported and
# writes the same stdout the step's `output:` captures. It is a file rather
# than an inline block so the recipe stays under its line budget; compressing
# the recipe instead is the one thing the brick rule forbids.

echo "=== STEP 18 VERIFICATION GATE ===" && \
echo "" && \
echo "Before proceeding, verify ALL of the following:" && \
echo "" && \
echo "  ✓ Did I address EVERY feedback comment (not just some)?" && \
echo "  ✓ Did I respond to each comment on the PR?" && \
echo "  ✓ Did I use the builder agent for implementation changes?" && \
echo "  ✓ Are all tests still passing?" && \
echo "" && \
echo "If any verification fails, STOP and complete the missing step." && \
echo "" && \
echo "=== GATE PASSED - Proceed to Step 19 ==="
