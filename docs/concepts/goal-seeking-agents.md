# Goal-Seeking Agents

A complete guide to generating, evaluating, and iterating on autonomous learning
agents in amplihack.

!!! note "Rust Port"
    In amplihack-rs, the goal-seeking agent system is orchestrated through the
    Rust CLI (`amplihack new`) and the agent framework crates. The underlying
    LLM interaction follows the same SDK-agnostic design as upstream.

---

## Table of Contents

- [What Are Goal-Seeking Agents?](#what-are-goal-seeking-agents)
- [Quick Start](#quick-start)
- [Generating Agents](#generating-agents)
- [Agent Capabilities](#agent-capabilities)
- [Architecture](#architecture)
- [Multi-Agent Architecture](#multi-agent-architecture)
- [Evaluating Agents](#evaluating-agents)
- [Iterating on Agents](#iterating-on-agents)
- [Domain Agents](#domain-agents)
- [Reference](#reference)

---

## What Are Goal-Seeking Agents?

Goal-seeking agents are autonomous programs that pursue objectives by learning,
reasoning, and taking actions. Unlike static scripts that follow a fixed
sequence, these agents:

1. **Learn** — Extract facts from content and store them in persistent memory.
2. **Remember** — Search, verify, and organize knowledge across sessions.
3. **Teach** — Explain what they know to other agents (or humans) through multi-turn dialogue.
4. **Apply** — Use stored knowledge and tools to solve new problems.

The system provides a single `GoalSeekingAgent` base class that works
identically across four different SDK backends (Copilot, Claude, Microsoft Agent
Framework, and a lightweight mini-framework). You write your agent logic once;
the SDK handles the underlying LLM calls, tool registration, and agent loop.

**Core design**: The `GoalSeekingAgent` ABC defines the interface. All SDK
implementations delegate learning and answering to a shared `LearningAgent`
instance, which contains the eval intelligence: LLM-based fact extraction,
intent detection, retrieval strategy selection, and answer synthesis.

### Related Contributor Docs

<!-- TODO: link when ported -->
- Understanding the LearningAgent module architecture
- LearningAgent module reference
- How to maintain and extend the refactored LearningAgent

---

## Quick Start

Generate and run a learning agent in under 5 minutes.

### 1. Write a goal prompt

Create a file describing what your agent should do:

```bash
cat > my_goal.md << 'EOF'
# Goal: Learn and Teach Python Testing

Learn best practices for Python testing (pytest, mocking, fixtures)
and be able to teach them to junior developers.

## Constraints
- Focus on pytest ecosystem
- Keep explanations beginner-friendly

## Success Criteria
- Can explain pytest fixtures with examples
- Can describe when to use mocking vs integration tests
- Can generate a test plan for a given module
EOF
```

### 2. Generate the agent

```bash
amplihack new --file my_goal.md --enable-memory --verbose
```

This produces a standalone agent directory:

```
goal_agents/learn-and-teach-python-testing/
  main.py              # Entry point
  README.md            # Agent documentation
  prompt.md            # Original goal
  agent_config.json    # Configuration
  .claude/
    agents/            # Matched skills
    context/
      goal.json        # Structured goal definition
      execution_plan.json
  logs/                # Runtime logs
```

### 3. Run the agent

```bash
cd goal_agents/learn-and-teach-python-testing
python main.py
```

### 4. Use the Python API directly

```python
# Upstream Python API (reference only)
from amplihack.agents.goal_seeking.sdk_adapters.factory import create_agent

agent = create_agent(
    name="my-learner",
    sdk="mini",
    instructions="You are a learning agent that acquires knowledge from content.",
    enable_memory=True,
)

agent.learn_from_content("React 20.1 was released in January 2026 with 47 new features.")
answer = agent.answer_question("When was React 20.1 released?")
print(answer)

agent.close()
```

---

## Generating Agents

### The `amplihack new` Command

The generator takes a natural language prompt file and produces a complete,
runnable agent directory.

**Pipeline stages:**

```
prompt.md
  │
  ▼
[1] Prompt Analysis     → GoalDefinition
  │
  ▼
[2] Objective Planning  → ExecutionPlan
  │
  ▼
[3] Skill Synthesis     → List[SkillDefinition]
  │
  ▼
[4] Agent Assembly      → GoalAgentBundle
  │
  ▼
[5] Packaging           → Standalone directory
```

### Command Options

| Option | Short | Description | Default |
|---|---|---|---|
| `--file` | `-f` | Path to prompt.md file (required) | — |
| `--output` | `-o` | Output directory | `./goal_agents` |
| `--name` | `-n` | Custom agent name | Auto-generated |
| `--skills-dir` | | Custom skills directory | `~/.amplihack/.claude/agents/amplihack` |
| `--verbose` | `-v` | Show detailed output | Off |
| `--enable-memory` | | Enable persistent memory | Off |
| `--sdk` | | SDK backend | `copilot` |
| `--multi-agent` | | Enable multi-agent architecture | Off |
| `--enable-spawning` | | Enable dynamic sub-agent creation | Off |

### Prompt Format

The generator accepts free-form markdown, but this structure produces the best
results:

```markdown
# Goal: <Primary objective in one sentence>

<Detailed description>

## Constraints
- Time limits, resource restrictions, scope boundaries

## Success Criteria
- Measurable outcomes that define "done"

## Context
- Background information, domain knowledge
```

---

## Agent Capabilities

### Learning

Agents extract facts from content and store them in persistent memory using
LLM-powered fact extraction (not simple keyword matching).

**Process:**

1. Content is passed to `learn_from_content()`
2. Temporal metadata is detected from the content
3. In hierarchical mode, an episode node is stored for provenance
4. The LLM extracts individual facts with confidence scores and tags
5. Facts are stored in the graph database with temporal metadata
6. A summary concept map is generated for knowledge organization

### Memory

Agents use the memory system for persistent knowledge storage:

- **Graph database** — Embedded graph DB, no external server required
- **Hierarchical memory** — Facts can supersede older facts via SUPERSEDES edges
- **Entity-centric indexing** — O(1) entity lookup via `entity_name` tags
- **Similarity search** — Keyword-boosted reranking
- **Cross-session persistence** — Knowledge survives between runs
- **Temporal metadata** — Tracks when facts were learned and source dates

**Seven learning tools are registered automatically:**

| Tool | Category | Description |
|---|---|---|
| `learn_from_content` | learning | Extract and store facts from text |
| `search_memory` | memory | Query stored knowledge by keyword/topic |
| `explain_knowledge` | teaching | Generate a topic explanation from stored facts |
| `find_knowledge_gaps` | learning | Identify what is unknown about a topic |
| `verify_fact` | applying | Check if a claim is consistent with stored knowledge |
| `store_fact` | memory | Directly store a fact with context and confidence |
| `get_memory_summary` | memory | Get statistics about what the agent knows |

### Answering Questions (Retrieval Cascade)

The `answer_question()` method uses a multi-step retrieval cascade:

1. **Intent detection** — Classifies the question into one of 9 intent types
2. **Retrieval strategy selection** — Entity-centric, simple+rerank, or Cypher aggregation
3. **Math pre-computation** — Extracts numbers and evaluates expressions safely
4. **Category-specific synthesis** — LLM prompt varies by intent type
5. **Math validation** — Checks arithmetic correctness
6. **Temporal code generation** — Code-based retrieval for temporal questions

### Teaching

The teaching system implements multi-turn dialogue between teacher and student
agents with **separate memory databases**. Knowledge transfer happens only
through conversation.

**Teaching strategy (informed by learning theory):**

1. **Advance Organizer** (Ausubel) — Structured overview
2. **Elaborative Interrogation** — Clarifying questions
3. **Scaffolding** (Vygotsky) — Adapted explanation level
4. **Self-Explanation** (Chi 1994) — Student summarizes understanding
5. **Reciprocal Teaching** (Palincsar & Brown) — Student teaches back
6. **Feynman Technique** — Every 5 exchanges, student teaches the material

---

## Architecture

### System Design

```
User Interface
  amplihack new --file goal.md --sdk copilot
                    │
                    ▼
      GoalSeekingAgent ABC (base)
      ├── learn_from_content()
      ├── answer_question()
      ├── form_goal(intent)
      ├── run(task, max_turns)
      └── close()
           │
    ┌──────┼──────────┬────────┐
    ▼      ▼          ▼        ▼
 Copilot  Claude   Microsoft  Mini
  SDK     Agent     Agent    Framework
  SDK     SDK     Framework
    └──────┼──────────┘────────┘
           ▼
   Shared LearningAgent Core
   ├── fact extraction
   ├── intent detection
   ├── retrieval strategies
   └── synthesis prompts
           │
           ▼
     Memory Library
   ├── Graph DB
   ├── Hierarchical memory
   └── Entity-centric indexing
```

---

## Multi-Agent Architecture

When `--multi-agent` is enabled, the generator creates a team of specialized
agents:

| Role | Responsibility |
|---|---|
| **Coordinator** | Decomposes goals, assigns tasks, tracks progress |
| **Researcher** | Gathers information from content and external sources |
| **Analyzer** | Processes and synthesizes gathered information |
| **Writer** | Produces final outputs (reports, code, documentation) |

---

## Evaluating Agents

### Progressive Evaluation Levels

Agents are evaluated on a 12-level progressive scale:

| Level | Name | What It Tests |
|---|---|---|
| L1 | Smoke | Agent starts and responds |
| L2 | Learning | Can extract and store facts |
| L3 | Recall | Can answer simple questions from memory |
| L4 | Synthesis | Can combine facts from multiple sources |
| L5 | Teaching | Can explain concepts to others |
| L6 | Temporal | Can reason about time-ordered events |
| L7 | Math | Can perform arithmetic on extracted numbers |
| L8 | Contradiction | Can detect conflicting information |
| L9 | Causal | Can reason about cause and effect |
| L10 | Meta-memory | Can answer questions about its own knowledge |
| L11 | Multi-agent | Can coordinate with other agents |
| L12 | Self-improvement | Can identify and fix its own weaknesses |

### Running Evaluations

```bash
# Run all levels
amplihack eval --agent-dir goal_agents/my-agent/

# Run specific level
amplihack eval --agent-dir goal_agents/my-agent/ --level L3

# Run with detailed output
amplihack eval --agent-dir goal_agents/my-agent/ --verbose
```

---

## Iterating on Agents

### Self-Improvement Loop

The self-improvement loop follows the EVAL → ANALYZE → RESEARCH → IMPROVE →
RE-EVAL → DECIDE cycle:

1. **EVAL**: Run evaluation suite, get baseline scores
2. **ANALYZE**: Identify weakest eval levels
3. **RESEARCH**: Generate hypotheses for improvement with evidence and counter-arguments
4. **IMPROVE**: Apply targeted improvements
5. **RE-EVAL**: Run evaluations again
6. **DECIDE**: Auto-commit if net improvement ≥ +2%, revert if regression > 5%

```bash
# Run self-improvement loop
amplihack improve --agent-dir goal_agents/my-agent/ --max-iterations 5
```

---

## Domain Agents

Pre-built domain agents for common use cases:

| Agent | Domain | Purpose |
|---|---|---|
| `security-scanner` | Security | Vulnerability detection and assessment |
| `dependency-auditor` | DevOps | Package dependency analysis |
| `api-doc-generator` | Documentation | API documentation from code |
| `test-coverage-analyzer` | Testing | Test gap identification |
| `pr-manager` | GitHub | Pull request management |
| `aks-sre` | Infrastructure | AKS reliability engineering |

See [Goal Agent Generator](agent-generator.md) for the generation system and
[Agent Lifecycle](agent-lifecycle.md) for lifecycle management.

---

## Reference

### GoalSeekingAgent Interface

```python
# Upstream Python API (reference only)
class GoalSeekingAgent(ABC):
    def learn_from_content(self, content: str) -> dict[str, Any]: ...
    def answer_question(self, question: str) -> str: ...
    async def run(self, task: str, max_turns: int = 10) -> AgentResult: ...
    def form_goal(self, user_intent: str) -> Goal: ...
    def get_memory_stats(self) -> dict[str, Any]: ...
    def close(self) -> None: ...
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `AMPLIHACK_AGENT_SDK` | `copilot` | Default SDK backend |
| `AMPLIHACK_AGENT_MEMORY` | `false` | Enable persistent memory |
| `AMPLIHACK_AGENT_VERBOSE` | `false` | Detailed agent output |
| `AMPLIHACK_AGENT_MAX_TURNS` | `10` | Maximum agent turns |

## Related Documentation

- [Goal Agent Generator](agent-generator.md) — generation pipeline
- [Agent Lifecycle](agent-lifecycle.md) — lifecycle management
- [Evaluation Framework](eval-framework.md) — evaluation system
- [Goal-Seeking Agent Tutorial](../howto/goal-seeking-agent-tutorial.md) — step-by-step guide
- [Example Goal Prompts](../reference/goal-agent-example-prompt.md) — prompt templates
