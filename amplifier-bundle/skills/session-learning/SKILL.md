---
name: session-learning
version: 1.0.0
description: |
  Cross-session learning system that extracts insights from session transcripts and injects
  relevant past learnings at session start. Uses simple keyword matching for relevance.
  Complements DISCOVERIES.md/PATTERNS.md with structured YAML storage.
invokes:
  - tools: [Read, Write, Edit, Grep, Glob]
  - commands: [/amplihack:learnings]
---

# Session Learning Skill

## Purpose

This skill provides cross-session learning by:

1. **Extracting** learnings from session transcripts at Stop hook
2. **Storing** learnings in structured YAML format (`~/.amplihack/.claude/data/learnings/`)
3. **Injecting** relevant past learnings at SessionStart based on task similarity
4. **Managing** learnings via `/amplihack:learnings` capability

## Design Philosophy

**Ruthlessly Simple Approach:**

- One YAML file per learning category (not per session)
- Simple keyword matching for relevance (no complex ML)
- Complements existing DISCOVERIES.md/PATTERNS.md - doesn't replace them
- Fail-safe: Never blocks session start or stop

## Learning Categories

Learnings are stored in five categories:

| Category         | File                | Purpose                              |
| ---------------- | ------------------- | ------------------------------------ |
| **errors**       | `errors.yaml`       | Error patterns and their solutions   |
| **workflows**    | `workflows.yaml`    | Workflow insights and shortcuts      |
| **tools**        | `tools.yaml`        | Tool usage patterns and gotchas      |
| **architecture** | `architecture.yaml` | Design decisions and trade-offs      |
| **debugging**    | `debugging.yaml`    | Debugging strategies and root causes |

## YAML Schema

Each learning file follows this structure:

```yaml
# .claude/data/learnings/errors.yaml
category: errors
last_updated: "2025-11-25T12:00:00Z"
learnings:
  - id: "err-001"
    created: "2025-11-25T12:00:00Z"
    keywords:
      - "import"
      - "module not found"
      - "circular dependency"
    summary: "Circular imports cause 'module not found' errors"
    insight: |
      When module A imports from module B and module B imports from module A,
      Python raises ImportError. Solution: Move shared code to a third module
      or use lazy imports.
    example: |
      # Bad: circular import
      # utils.py imports from models.py
      # models.py imports from utils.py

      # Good: extract shared code
      # shared.py has common functions
      # both utils.py and models.py import from shared.py
    confidence: 0.9
    times_used: 3
```

## When to Use This Skill

**Automatic Usage (via hooks):**

- At session stop: Extracts learnings from transcript
- At session start: Injects relevant learnings based on prompt keywords

**Manual Usage:**

- When you want to view/manage learnings
- When debugging and want to recall past solutions
- When onboarding to understand project-specific patterns

## Learning Extraction Process

### Step 1: Analyze Session Transcript

At session stop, scan for:

1. **Error patterns**: Errors encountered and how they were solved
2. **Workflow insights**: Steps that worked well or poorly
3. **Tool discoveries**: New ways of using tools effectively
4. **Architecture decisions**: Design choices and their rationale
5. **Debugging strategies**: Root cause analysis patterns

### Step 2: Extract Structured Learning

For each significant insight:

1. Generate unique ID based on category and timestamp
2. Extract keywords from context (3-5 relevant terms)
3. Create one-sentence summary
4. Write detailed insight with explanation
5. Include code example if applicable
6. Assign confidence score (0.5-1.0)

### Step 3: Merge with Existing Learnings

1. Check for duplicate learnings using keyword overlap
2. If similar learning exists (>60% keyword match), update confidence
3. Otherwise, append new learning to category file

## Learning Injection Process

### Step 1: Extract Task Keywords

From session start prompt, extract:

- Technical terms (languages, frameworks, tools)
- Problem indicators (error, fix, debug, implement)
- Domain keywords (api, database, auth, etc.)

### Step 2: Find Relevant Learnings

For each learning category:

1. Load learnings from YAML
2. Calculate keyword overlap with task
3. Rank by `overlap_score * confidence * recency_weight`
4. Select top 3 most relevant learnings

### Step 3: Inject Context

Format relevant learnings as context:

```markdown
## Past Learnings Relevant to This Task

### [Category]: [Summary]

## [Insight with example if helpful]
```

## Usage Examples

### Example 1: Automatic Extraction

```
Session: Debugging circular import issue in Neo4j module
Duration: 45 minutes
Resolution: Moved shared types to separate file

Extracted Learning:
- Category: errors
- Keywords: [import, circular, neo4j, type]
- Summary: Circular imports in Neo4j types cause ImportError
- Insight: When Neo4jNode imports from connection.py which imports
  Node types, move types to separate types.py module
- Example: types.py with dataclasses, connection.py imports from types.py
```

### Example 2: Automatic Injection

```
Session Start Prompt: "Fix the import error in the memory module"

Matched Learnings:
1. errors/err-001: "Circular imports cause 'module not found' errors" (85% match)
2. debugging/dbg-003: "Use `python -c` to isolate import issues" (60% match)

Injected Context:
## Past Learnings Relevant to This Task

### Errors: Circular imports cause 'module not found' errors
When module A imports from module B and B imports from A, Python raises
ImportError. Solution: Move shared code to a third module or use lazy imports.
---
```

### Example 3: Manual Management

```
User: Show me what I've learned about testing

Claude (using this skill):
1. Reads .claude/data/learnings/workflows.yaml
2. Filters learnings with keywords containing "test"
3. Displays formatted list with summaries and examples
```

## Keyword Matching Algorithm

Simple but effective matching:

```python
def calculate_relevance(task_keywords: set, learning_keywords: set) -> float:
    """Calculate relevance score between 0 and 1."""
    if not task_keywords or not learning_keywords:
        return 0.0

    # Count overlapping keywords
    overlap = task_keywords & learning_keywords

    # Score: overlap / min(task, learning) to not penalize short queries
    return len(overlap) / min(len(task_keywords), len(learning_keywords))
```

## Integration Points

### With Stop Hook

The stop hook can call this skill to extract learnings:

1. Parse transcript for significant events
2. Identify error patterns, solutions, insights
3. Store in appropriate category YAML
4. Log extraction summary

### With Session Start Hook

The session start hook can inject relevant learnings:

1. Parse initial prompt for keywords
2. Find matching learnings across categories
3. Format as context injection
4. Include in session context

### With /amplihack:learnings Command

Command interface for learning management:

- `/amplihack:learnings show [category]` - Display learnings
- `/amplihack:learnings search <query>` - Search across all categories
- `/amplihack:learnings add` - Manually add a learning
- `/amplihack:learnings stats` - Show learning statistics

## Quality Guidelines

### When to Extract

Extract a learning when:

- Solving a problem that took >10 minutes
- Discovering non-obvious tool behavior
- Finding a pattern that applies broadly
- Making an architecture decision with trade-offs

### When NOT to Extract

Skip extraction when:

- Issue was trivial typo or syntax error
- Solution is already in DISCOVERIES.md or PATTERNS.md
- Insight is too project-specific to reuse
- Confidence is low (<0.5)

### Learning Quality Checklist

- [ ] Keywords are specific and searchable
- [ ] Summary is one clear sentence
- [ ] Insight explains WHY, not just WHAT
- [ ] Example is minimal and runnable
- [ ] Confidence reflects actual certainty

## File Locations

```
.claude/
  data/
    learnings/
      errors.yaml        # Error patterns and solutions
      workflows.yaml     # Workflow insights
      tools.yaml         # Tool usage patterns
      architecture.yaml  # Design decisions
      debugging.yaml     # Debugging strategies
      _stats.yaml        # Usage statistics (auto-generated)
```

## Comparison with Existing Systems

| Feature   | DISCOVERIES.md    | PATTERNS.md     | Session Learning   |
| --------- | ----------------- | --------------- | ------------------ |
| Format    | Markdown          | Markdown        | YAML               |
| Audience  | Humans            | Humans          | Agents + Humans    |
| Storage   | Single file       | Single file     | Per-category files |
| Matching  | Manual read       | Manual read     | Keyword-based auto |
| Injection | Manual            | Manual          | Automatic          |
| Scope     | Major discoveries | Proven patterns | Any useful insight |

**Complementary Use:**

- Use DISCOVERIES.md for major, well-documented discoveries
- Use PATTERNS.md for proven, reusable patterns with code
- Use Session Learning for quick insights that help future sessions

## Error Handling

### YAML Parsing Errors

If a learning file becomes corrupted or invalid:

```python
import yaml
from pathlib import Path

def safe_load_learnings(filepath: Path) -> dict:
    """Load learnings with graceful error handling."""
    try:
        content = filepath.read_text()
        data = yaml.safe_load(content)
        if not isinstance(data, dict) or "learnings" not in data:
            print(f"Warning: Invalid structure in {filepath}, using empty learnings")
            return {"category": filepath.stem, "learnings": []}
        return data
    except yaml.YAMLError as e:
        print(f"Warning: YAML error in {filepath}: {e}")
        # Create backup before recovery
        backup = filepath.with_suffix(".yaml.bak")
        filepath.rename(backup)
        print(f"Backed up corrupted file to {backup}")
        return {"category": filepath.stem, "learnings": []}
    except Exception as e:
        print(f"Warning: Could not read {filepath}: {e}")
        return {"category": filepath.stem, "learnings": []}
```

### Missing Files

If the learnings directory doesn't exist, create it:

```python
def ensure_learnings_directory():
    """Create learnings directory and empty files if missing."""
    learnings_dir = Path(".claude/data/learnings")
    learnings_dir.mkdir(parents=True, exist_ok=True)

    categories = ["errors", "workflows", "tools", "architecture", "debugging"]
    for cat in categories:
        filepath = learnings_dir / f"{cat}.yaml"
        if not filepath.exists():
            filepath.write_text(f"category: {cat}\nlearnings: []\n")
```

### Fail-Safe Principle

The learning system follows fail-safe design:

- **Never blocks session start**: If injection fails, session continues normally
- **Never blocks session stop**: If extraction fails, session ends normally
- **Logs warnings but continues**: Errors are logged, not raised
- **Creates backups before modifications**: Corrupt files are preserved

## Hook Integration

### Stop Hook: Learning Extraction

Add learning extraction to your stop hook (implemented in `crates/amplihack-hooks/src/stop/`):

```rust
// Conceptual illustration of the learning extraction flow.
// See crates/amplihack-hooks/src/stop/ for the actual Rust implementation.

/// Extract learnings from session transcript at stop.
async fn extract_session_learnings(transcript: &str, session_id: &str) -> Result<()> {
    // Only extract if session was substantive (not just a quick question)
    if transcript.len() < 1000 {
        return Ok(());
    }

    let extraction_prompt = format!(
        "Analyze this session transcript and extract any reusable learnings.\n\
         Categories: errors, workflows, tools, architecture, debugging\n\
         For each learning, provide: category, keywords, summary, insight, confidence.\n\
         Transcript:\n{}", &transcript[..transcript.len().min(5000)]
    );

    // ... call LLM to extract ...
    // ... parse response and store in learning YAML files ...
    Ok(())
}
```

### Session Start Hook: Learning Injection

Add learning injection to your session start hook (implemented in `crates/amplihack-hooks/src/session_start/`):

```rust
// Conceptual illustration of the learning injection flow.
// See crates/amplihack-hooks/src/session_start/ for the actual Rust implementation.

/// Find and format relevant learnings for injection.
fn inject_relevant_learnings(initial_prompt: &str) -> String {
    let learnings_dir = PathBuf::from(".claude/data/learnings");
    if !learnings_dir.exists() {
        return String::new();
    }

    // Extract keywords from prompt, find matching learnings,
    // return top 3 matches formatted as context injection.
    // ...

    format!("## Past Learnings Relevant to This Task\n\n{}", matches_text)
}
```

## Limitations

1. **Keyword matching is imperfect** - May miss relevant learnings or match irrelevant ones
2. **No semantic understanding** - Can't match conceptually similar but differently-worded insights
3. **Storage is local** - Learnings don't sync across machines
4. **Manual cleanup needed** - Old/wrong learnings should be periodically reviewed

## Future Improvements

If needed, consider:

- Embedding-based similarity for better matching
- Cross-machine sync via git
- Automatic confidence decay over time
- Integration with Neo4j for graph-based learning relationships

## Success Metrics

Track effectiveness:

- **Injection rate**: % of sessions with relevant learning injected
- **Usage rate**: How often injected learnings help solve problems
- **Growth rate**: New learnings per week
- **Quality**: User feedback on learning relevance
