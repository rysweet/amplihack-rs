# Eval Grading Improvements

**Type**: Explanation (Understanding-Oriented)

How grader false negatives were fixed and advanced retrieval strategies
implemented to improve evaluation accuracy.

## The Problem: Grader False Negatives

Consider this example:

- **Question**: "What is the current project budget?"
- **Expected**: "$1.4M"
- **Agent answer**: "The budget increased from $1.2M to $1.4M"

The agent's answer is correct ($1.4M is present), but a naive grader penalizes it
because the historical value $1.2M matches an "incorrect pattern." This is a
**false negative** — correct answers scored as wrong.

### Root Cause

The grader checked for incorrect patterns WITHOUT first verifying that the
correct answer was present:

```python
# BUGGY: penalizes even when correct answer IS present
if any(pattern in answer.lower() for pattern in incorrect_patterns):
    score = 0.0
```

## The Fix: Correct-First Grading

Only apply incorrect pattern penalties when the correct keywords are NOT present:

```python
def _deterministic_grade(answer: str, rubric: GradingRubric) -> float:
    answer_lower = answer.lower()

    # Check if correct keywords are present FIRST
    has_correct = any(
        keyword.lower() in answer_lower
        for keyword in rubric.correct_keywords
    )

    # Only penalize incorrect patterns if correct answer is MISSING
    if not has_correct:
        for pattern in rubric.incorrect_patterns:
            if pattern.lower() in answer_lower:
                return 0.0

    # Grade based on keyword match ratio
    matches = sum(
        1 for keyword in rubric.correct_keywords
        if keyword.lower() in answer_lower
    )
    return matches / len(rubric.correct_keywords)
```

### Key Insight

An answer containing both historical and current values should score based on
whether the **current** (correct) value is present — not penalized for also
mentioning the historical value.

## Entity-Linked Retrieval

Standard context-based search misses questions targeting structured entity IDs
(e.g., `INC-2024-089`, `CVE-2024-12345`). Entity-linked retrieval extracts these
IDs and searches across ALL contexts.

### How It Works

1. Extract entity IDs from question using regex patterns
2. For each entity ID, search ALL memory contexts (not just the current one)
3. Aggregate and deduplicate retrieved facts
4. Return enriched fact list

### Supported Entity ID Patterns

| Pattern          | Example        | Use Case                    |
| ---------------- | -------------- | --------------------------- |
| `INC-YYYY-NNN`  | INC-2024-089   | Security incidents, outages |
| `CVE-YYYY-NNNNN`| CVE-2024-12345 | Vulnerability tracking      |
| `PROJ-NNN`      | PROJ-456       | Project management          |
| `SRV-NNN`       | SRV-789        | Infrastructure inventory    |

## Multi-Entity Retrieval

For questions requiring multi-hop reasoning across multiple entities:

1. Identify all named entities in the question
2. Retrieve facts for each entity independently
3. Combine with context overlap detection
4. Support cross-entity relationship traversal

This handles questions like "Compare the response times of INC-2024-089 and
INC-2024-102" where facts about both incidents must be retrieved.

## Results

These improvements increased evaluation accuracy:

- Overall: 96.0% to 97.8% (+1.8%)
- Temporal evolution: 86.6% to 99.8% (+13.2%)
- Security log analysis: 88% to 100% (+12%)
- 9 categories reached 100% accuracy

## Best Practices

1. **Deterministic before semantic** — Check for exact/pattern matches first;
   fall back to LLM grading only for ambiguous cases
2. **Correct-first logic** — Always check for correct answers before penalizing
   for incorrect patterns
3. **Cross-context retrieval** — For structured IDs, search all contexts,
   not just the current one
4. **Multi-vote grading** — Use 3+ grading votes and take the median

## Related

- [Eval System Architecture](../concepts/eval-system-architecture.md) — full evaluation system overview
- [Eval Retrieval Reference](../reference/eval-retrieval-reference.md) — retrieval method specifications
