# Eval Retrieval Methods Reference

**Type**: Reference (Information-Oriented)

Specifications for retrieval methods in the amplihack evaluation system.
These methods improve fact retrieval accuracy for structured entity IDs and
multi-hop reasoning questions.

## Method Comparison

| Method                     | Use Case              | Multi-Entity | Complexity                        |
| -------------------------- | --------------------- | ------------ | --------------------------------- |
| `_entity_linked_retrieval` | Structured entity IDs | No           | O(n x e) where n=facts, e=entities |
| `_multi_entity_retrieval`  | Multi-hop reasoning   | Yes          | O(n x e x k) where k=facts/entity  |
| `_standard_retrieval`      | General questions     | No           | O(n)                              |

## Entity-Linked Retrieval

### Signature

```python
def _entity_linked_retrieval(
    self,
    question: str,
    limit_per_entity: int = 10
) -> list[Fact]:
```

**Parameters:**

| Parameter          | Type | Default | Description                     |
| ------------------ | ---- | ------- | ------------------------------- |
| `question`         | str  | —       | Question text with entity IDs   |
| `limit_per_entity` | int  | 10      | Max facts retrieved per entity  |

**Returns:** Deduplicated list of facts containing the entity IDs.

### Supported Entity ID Patterns

| Pattern            | Regex                          | Example        |
| ------------------ | ------------------------------ | -------------- |
| Incident           | `INC-\d{4}-\d{3}`             | INC-2024-089   |
| CVE                | `CVE-\d{4}-\d{4,5}`           | CVE-2024-12345 |
| Project            | `PROJ-\d{3}`                   | PROJ-456       |
| Server             | `SRV-\d{3}`                    | SRV-789        |

### Algorithm

1. Extract entity IDs from question using regex patterns
2. For each entity ID:
   - Search memory for facts containing the entity ID text
   - Search across ALL contexts (not limited to single context)
   - Retrieve up to `limit_per_entity` facts
3. Aggregate all retrieved facts
4. Deduplicate based on fact ID
5. Return deduplicated fact list

### When to Use

- Question contains structured identifiers (INC-, CVE-, PROJ-, SRV-)
- Standard retrieval misses because the ID is not in the current context
- Facts about the entity are distributed across multiple learning sessions

## Multi-Entity Retrieval

### Signature

```python
def _multi_entity_retrieval(
    self,
    question: str,
    limit_per_entity: int = 5
) -> list[Fact]:
```

**Parameters:**

| Parameter          | Type | Default | Description                     |
| ------------------ | ---- | ------- | ------------------------------- |
| `question`         | str  | —       | Question with multiple entities |
| `limit_per_entity` | int  | 5       | Max facts retrieved per entity  |

**Returns:** Combined fact list from all identified entities.

### Algorithm

1. Identify all named entities in the question
2. For each entity:
   - Perform independent fact retrieval
   - Include cross-context results
3. Combine results with overlap detection
4. Return merged fact list

### When to Use

- Question requires comparing or relating multiple entities
- Multi-hop reasoning where facts from separate entities must be combined
- Example: "Compare the response times of INC-2024-089 and INC-2024-102"

## Standard Retrieval

### Signature

```python
def _standard_retrieval(
    self,
    question: str,
    context: str | None = None,
    limit: int = 10
) -> list[Fact]:
```

**Parameters:**

| Parameter | Type         | Default | Description              |
| --------- | ------------ | ------- | ------------------------ |
| `question` | str         | —       | Question text            |
| `context` | str or None  | None    | Optional context filter  |
| `limit`   | int          | 10      | Max facts to retrieve    |

**Returns:** Facts matching the question within the specified context.

### When to Use

- General questions without structured entity IDs
- Single-context retrieval is sufficient
- Default retrieval path when no special patterns detected

## Retrieval Strategy Selection

The system automatically selects the retrieval method:

```
1. Check question for structured entity IDs (INC-, CVE-, PROJ-, SRV-)
   -> If found: use entity_linked_retrieval

2. Check if question references multiple named entities
   -> If yes: use multi_entity_retrieval

3. Default: use standard_retrieval
```

## Related

- [Eval System Architecture](../concepts/eval-system-architecture.md) — full system overview
- [Eval Grading Improvements](../concepts/eval-grading-improvements.md) — grading fix details
- [Evaluation Framework](../concepts/eval-framework.md) — high-level framework concepts
