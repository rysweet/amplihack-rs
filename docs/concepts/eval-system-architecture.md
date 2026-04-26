# Eval System Architecture

**Type**: Explanation (Understanding-Oriented)

Comprehensive guide to the evaluation and self-improvement infrastructure for
goal-seeking agents. Covers the progressive test suite (L1-L12), long-horizon
memory testing, multi-SDK evaluation, and the self-improvement loop.

## Overview

The eval system is a multi-layered framework that tests agent learning and
reasoning capabilities across 12 progressively harder levels. It supports
multiple SDK implementations, includes a self-improvement loop with patch
proposer and reviewer voting, and provides domain-specific evaluation for
specialized agents.

## Architecture

```
+------------------------------------------------------------------+
|                    EVALUATION ENTRY POINTS                         |
+------------------------------------------------------------------+
|                                                                    |
|  progressive_test_suite     sdk_eval_loop       run_domain_evals  |
|  (L1-L12 single/parallel)  (multi-SDK loop)    (domain agents)   |
|                                                                    |
|  self_improve/runner        long_horizon_memory                   |
|  (closed-loop improvement)  (1000-turn stress test, 12 blocks)   |
|                                                                    |
+------------------------------------------------------------------+
                        |                |
                        v                v
+------------------------------------------------------------------+
|                    CORE EVAL PIPELINE                              |
+------------------------------------------------------------------+
|                                                                    |
|  1. DATA LAYER              2. AGENT LAYER                        |
|  - test_levels (L1-L12)     - subprocess isolation                |
|  - TestArticle/Question     - learning phase                      |
|  - long_horizon_data        - testing phase                       |
|    (12-block generation)    - SDK routing                         |
|                                                                    |
|  3. GRADING LAYER                                                 |
|  - LLM semantic grading     - metacognition grading               |
|  - multi-vote scoring       - level-specific rubrics              |
|  - deterministic fallback   - 4-dimension scoring                 |
|                                                                    |
+------------------------------------------------------------------+
                        |
                        v
+------------------------------------------------------------------+
|                    ANALYSIS & IMPROVEMENT                          |
+------------------------------------------------------------------+
|                                                                    |
|  error_analyzer (10 failure modes)                                |
|  patch_proposer (LLM-generated diffs)                             |
|  reviewer_voting (3-perspective review)                           |
|                                                                    |
|  EVAL -> ANALYZE -> PROPOSE -> CHALLENGE -> VOTE ->               |
|    APPLY -> RE-EVAL -> DECIDE                                     |
|                                                                    |
+------------------------------------------------------------------+
```

## Progressive Test Suite (L1-L12)

| Level | Name                     | Description                           |
| ----- | ------------------------ | ------------------------------------- |
| L1    | Direct Recall            | Single source direct recall           |
| L2    | Multi-Source Synthesis   | Combine facts across sources          |
| L3    | Temporal Reasoning       | Track changes over time               |
| L4    | Procedural Learning      | Learn and apply procedures            |
| L5    | Contradiction Handling   | Resolve conflicting information       |
| L6    | Incremental Learning     | Accumulate knowledge over time        |
| L7    | Teacher-Student Transfer | Teach learned knowledge to another    |
| L8    | Analogical Reasoning     | Apply patterns to novel domains       |
| L9    | Meta-Cognitive           | Reason about own knowledge gaps       |
| L10   | Adversarial              | Resist misleading information         |
| L11   | Long-Range Dependencies  | Connect distant facts                 |
| L12   | Open-Ended Synthesis     | Generate novel insights               |

Each level runs in a separate subprocess with its own memory database,
preventing cross-level contamination.

## Long-Horizon Memory Testing

Stress tests agent memory across 1,000 turns using 12 information blocks
(including security domain). Measures retention, update handling, and retrieval
accuracy at scale.

## Self-Improvement Loop

An 8-stage cycle per iteration:

1. **EVAL** — Run evaluation to get per-category scores
2. **ANALYZE** — Identify worst-performing category
3. **PROPOSE** — Generate unified diff with hypothesis and expected impact
4. **CHALLENGE** — Devil's advocate arguments against the patch
5. **VOTE** — Three reviewer perspectives (quality, regression, simplicity)
6. **APPLY** — If accepted, apply patch and commit
7. **RE-EVAL** — Run evaluation again to measure impact
8. **DECIDE** — If regression > 5% on any category, auto-revert; if net improvement >= 2%, keep

`PatchHistory` tracks all applied, reverted, and rejected patches to prevent
repeating failed fixes.

## Error Analysis Taxonomy

| Failure Mode               | Description                    |
| -------------------------- | ------------------------------ |
| retrieval_insufficient     | Not enough facts retrieved     |
| temporal_ordering_wrong    | Wrong temporal arithmetic      |
| intent_misclassification   | Wrong intent detection         |
| fact_extraction_incomplete | Missing facts in memory        |
| synthesis_hallucination    | Invented information           |
| update_not_applied         | Used outdated data             |
| contradiction_undetected   | Missed conflicting sources     |
| procedural_ordering_lost   | Steps out of sequence          |
| teaching_coverage_gap      | Student not taught key facts   |
| counterfactual_refusal     | Refused hypothetical reasoning |

## General Capability Evaluation

Beyond memory, 5 general-purpose capabilities are evaluated:

| Eval Type                   | What It Tests                              | Scenarios |
| --------------------------- | ------------------------------------------ | --------- |
| Tool Use Efficiency         | Correct tool selection, chaining, economy  | 4         |
| Planning                    | Multi-step task decomposition              | 3         |
| Reasoning Under Uncertainty | Handling incomplete/conflicting evidence    | 3         |
| Cross-Domain Transfer       | Applying patterns to new domains           | 2         |
| Collaborative Task          | Multi-agent delegation and synthesis       | 2         |

## Key Design Decisions

1. **Subprocess Isolation** — Each eval level runs in a separate subprocess with
   its own memory database, preventing cross-level contamination and ensuring
   reproducibility.

2. **LLM-Based Grading** — Uses semantic grading rather than exact-match scoring,
   handling equivalences like "26 medals" vs "twenty-six medals" and partial credit.

3. **Multi-Vote Grading** — Each answer is graded 3 times independently; the
   median reduces noise on ambiguous answers.

4. **3-Run Medians** — Single runs are unreliable due to LLM stochasticity.
   Running 3 times and taking median scores gives stable results.

## amplihack-rs Integration

In amplihack-rs, the eval system is accessed via:

```bash
# Run progressive test suite
amplihack agent-eval --levels L1,L2,L3

# Run with specific SDK
amplihack agent-eval --sdk mini --output-dir ./eval-results
```

The `crates/amplihack-agent-eval/` crate provides the Rust evaluation harness,
while the Python eval scripts in `amplifier-bundle/` handle LLM-based grading.

See [Evaluation Framework](../concepts/eval-framework.md) for the high-level
framework overview and [Run Agent Evaluations](../howto/run-agent-evaluations.md)
for step-by-step instructions.

## Related

- [Eval Grading Improvements](../concepts/eval-grading-improvements.md) — fixing grader false negatives
- [Eval Retrieval Reference](../reference/eval-retrieval-reference.md) — retrieval method specifications
- [Evaluation Framework](../concepts/eval-framework.md) — high-level framework overview
