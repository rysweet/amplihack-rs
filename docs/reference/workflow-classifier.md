# Workflow Classifier — Reference

The workflow classifier examines a task description and routes it to the appropriate workflow type. This reference documents the classification keywords, priority rules, and edge-case handling.

## Contents

- [Workflow types](#workflow-types)
- [Keyword tables](#keyword-tables)
- [Classification algorithm](#classification-algorithm)
- [Constructive-verb disambiguation](#constructive-verb-disambiguation)
- [Examples](#examples)

---

## Workflow types

| Type | Purpose | Typical trigger phrases |
|------|---------|------------------------|
| `Default` | Feature development, bug fixes, refactoring | "add", "create", "fix", "implement", "refactor" |
| `Investigation` | Research, exploration, understanding existing code | "investigate", "analyze", "understand", "explore" |
| `Ops` | System maintenance, cleanup, file management | "disk cleanup", "manage repos", "delete files" |
| `Verification` | Trivial changes, config tweaks | "update version", "change constant" |

---

## Keyword tables

### Default workflow keywords

General development verbs match `Default` by default. No explicit keyword list is needed — `Default` is the fallback when no other workflow matches with higher confidence.

### Ops workflow keywords

OPS keywords are multi-word phrases. Single generic words are not used because they cause false positives when they appear inside code paths or task descriptions for development work.

| Keyword phrase | Example match |
|----------------|---------------|
| `run command` | "run command to restart service" |
| `disk cleanup` | "disk cleanup on staging servers" |
| `repo management` | "repo management for archived projects" |
| `git operations` | "git operations to prune old branches" |
| `delete files` | "delete files from tmp directory" |
| `organize files` | "organize files into date folders" |
| `clean up temp` | "clean up temp directories on build box" |
| `manage repos` | "manage repos for team onboarding" |

**Why multi-word phrases:** Single words like `cleanup` matched as substrings in code paths (e.g., `cmd_cleanup.rs`) and development task descriptions ("Add an agentic disk-cleanup loop"), causing development tasks to be misrouted to the Ops workflow.

---

## Classification algorithm

```
1. Lowercase the input task description
2. For each workflow type (in priority order):
   a. Count keyword phrase matches in the input
   b. Weight by phrase specificity (longer phrases score higher)
3. Return the workflow type with the highest weighted score
4. If no keywords match, return Default
```

Priority order when scores tie: `Investigation` > `Ops` > `Verification` > `Default`.

---

## Constructive-verb disambiguation

When a task description contains both OPS keyword phrases and constructive development verbs, the classifier favors `Default`. This prevents development tasks that mention operational concepts from being misrouted.

Constructive verbs that override OPS classification:

- `add`, `create`, `build`, `implement`, `write`, `design`, `develop`, `make`, `extend`, `refactor`

**Example:**

```
Input:  "Add an agentic disk-cleanup loop. Extend src/cmd_cleanup.rs."
Ops match:  "disk cleanup" (1 match)
Override:   "Add" and "Extend" are constructive verbs
Result:     Default (constructive verb override wins)
```

---

## Examples

| Input | Classification | Reason |
|-------|---------------|--------|
| `"Fix the login bug in auth.rs"` | Default | No OPS/Investigation keywords |
| `"disk cleanup on staging servers"` | Ops | Matches "disk cleanup" phrase |
| `"Add an agentic disk-cleanup loop"` | Default | "disk cleanup" matches OPS, but "Add" is constructive |
| `"investigate why tests fail on CI"` | Investigation | Matches investigation keywords |
| `"manage repos for team onboarding"` | Ops | Matches "manage repos" phrase |
| `"Implement repo management feature"` | Default | "repo management" matches OPS, but "Implement" is constructive |

---

## Source

`crates/amplihack-workflows/src/classifier.rs`

## Related

- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — How recipes invoke workflow classification
- [amplihack recipe](./recipe-command.md) — CLI reference for recipe execution
