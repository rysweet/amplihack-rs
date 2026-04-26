# Goal-Seeking Agent Generator Tutorial

A step-by-step guide to generating, evaluating, and iterating on autonomous
learning agents in amplihack.

!!! note "Rust Port"
    In amplihack-rs, the generator CLI is `amplihack new`. The tutorial
    steps are the same; only the installation and binary invocation differ.

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Your First Agent](#2-your-first-agent)
3. [SDK Selection Guide](#3-sdk-selection-guide)
4. [Multi-Agent Architecture](#4-multi-agent-architecture)
5. [Running Evaluations](#5-running-evaluations)
6. [Understanding Eval Levels](#6-understanding-eval-levels)
7. [Self-Improvement Loop](#7-self-improvement-loop)
8. [Troubleshooting](#troubleshooting)

---

## 1. Introduction

### What Is a Goal-Seeking Agent?

A goal-seeking agent is an autonomous program that pursues an objective by
**learning**, **remembering**, **teaching**, and **applying** knowledge. Unlike
a static script that follows a fixed sequence, these agents adapt and improve.

### Architecture

The generator pipeline has five stages:

```
Prompt (.md)
    │
    ▼
PromptAnalyzer → GoalDefinition
    │
    ▼
ObjectivePlanner → ExecutionPlan
    │
    ▼
SkillSynthesizer → Skills + SDK Tools
    │
    ▼
AgentAssembler → GoalAgentBundle
    │
    ▼
GoalAgentPackager → /goal_agents/<name>/
```

1. **Analyze**: Extract goal, domain, constraints from markdown
2. **Plan**: Break goal into phases with capabilities
3. **Synthesize**: Match skills and SDK-native tools
4. **Assemble**: Build the agent bundle with config and metadata
5. **Package**: Write to disk as a runnable project

### The GoalSeekingAgent Interface

Every generated agent implements the same interface regardless of SDK:

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

Write your agent logic once; swap SDKs freely.

---

## 2. Your First Agent

### Step 1: Write a Prompt File

Create `my_goal.md`:

```markdown
# Goal: Learn and Summarize Python Best Practices

## Objective
Build an agent that reads Python style guides and can answer
questions about best practices.

## Domain
software-engineering

## Constraints
- Focus on PEP-8 and type-hinting
- Keep answers concise

## Success Criteria
- Can explain PEP-8 naming conventions
- Can describe when to use type hints
```

The prompt file requires four sections: **Goal/Objective**, **Domain**,
**Constraints**, and **Success Criteria**.

### Step 2: Generate the Agent

```bash
amplihack new --file my_goal.md
```

With custom output directory and name:

```bash
amplihack new --file my_goal.md --name python-coach --output ./agents
```

### Step 3: Run the Agent

```bash
cd goal_agents/python-coach
python main.py
```

### What Happens Under the Hood

1. `PromptAnalyzer` parses `my_goal.md` and extracts goal, domain, constraints
2. `ObjectivePlanner` creates an `ExecutionPlan` with phases
3. `SkillSynthesizer` matches skills from `.claude/agents/amplihack/`
4. `AgentAssembler` builds the `GoalAgentBundle`
5. `GoalAgentPackager` writes files to disk

---

## 3. SDK Selection Guide

Choose an SDK based on your needs:

| SDK | LLM | Best For | Native Tools |
|---|---|---|---|
| `copilot` | GPT-4.1 | GitHub integration, file ops | file, git, web |
| `claude` | Claude Sonnet | Code analysis, writing | bash, read, write, grep |
| `microsoft` | GPT-4o | Enterprise, session mgmt | FunctionTool |
| `mini` | Any (via API) | Lightweight, testing | Learning tools only |

### Using a Specific SDK

```bash
# Generate with Claude SDK
amplihack new --file my_goal.md --sdk claude

# Generate with Copilot SDK
amplihack new --file my_goal.md --sdk copilot

# Generate with Mini framework (lightweight)
amplihack new --file my_goal.md --sdk mini
```

---

## 4. Multi-Agent Architecture

Enable multi-agent mode for complex goals:

```bash
amplihack new --file my_goal.md --multi-agent
```

This generates a team of specialized agents:

| Role | Responsibility |
|---|---|
| **Coordinator** | Decomposes goals, assigns tasks |
| **Researcher** | Gathers information |
| **Analyzer** | Processes and synthesizes |
| **Writer** | Produces final outputs |

### Agent Spawning

Enable dynamic sub-agent creation:

```bash
amplihack new --file my_goal.md --multi-agent --enable-spawning
```

With spawning, the coordinator can create new specialized agents at runtime
based on the task requirements.

---

## 5. Running Evaluations

### Basic Evaluation

```bash
# Run all evaluation levels
amplihack eval --agent-dir goal_agents/my-agent/

# Run specific level
amplihack eval --agent-dir goal_agents/my-agent/ --level L3

# Run with detailed output
amplihack eval --agent-dir goal_agents/my-agent/ --verbose
```

### Evaluation Output

```
Evaluation Results for: python-coach
═══════════════════════════════════

L1  Smoke .............. PASS  (1.2s)
L2  Learning ........... PASS  (3.4s)
L3  Recall ............. PASS  (2.1s)
L4  Synthesis .......... PASS  (4.5s)
L5  Teaching ........... FAIL  (5.2s)
    └─ Could not explain pytest fixtures with examples
L6  Temporal ........... SKIP  (depends on L5)

Overall: 4/6 passed (66.7%)
```

---

## 6. Understanding Eval Levels

| Level | Name | What It Tests | Pass Criteria |
|---|---|---|---|
| L1 | Smoke | Agent starts and responds | Returns non-empty response |
| L2 | Learning | Can extract and store facts | ≥ 1 fact stored |
| L3 | Recall | Can answer simple questions | Correct answer from memory |
| L4 | Synthesis | Combines facts from multiple sources | Multi-source answer |
| L5 | Teaching | Can explain concepts | Clear, accurate explanation |
| L6 | Temporal | Reasons about time-ordered events | Correct temporal ordering |
| L7 | Math | Performs arithmetic on extracted numbers | Correct computation |
| L8 | Contradiction | Detects conflicting information | Identifies conflict |
| L9 | Causal | Reasons about cause and effect | Correct causal chain |
| L10 | Meta-memory | Answers questions about its knowledge | Accurate self-assessment |
| L11 | Multi-agent | Coordinates with other agents | Successful delegation |
| L12 | Self-improvement | Identifies and fixes weaknesses | Score improvement |

---

## 7. Self-Improvement Loop

The self-improvement loop automatically iterates on agent quality:

```bash
amplihack improve --agent-dir goal_agents/my-agent/ --max-iterations 5
```

### Cycle

1. **EVAL**: Run evaluation suite, get baseline scores
2. **ANALYZE**: Identify weakest eval levels
3. **RESEARCH**: Generate hypotheses for improvement
4. **IMPROVE**: Apply targeted improvements
5. **RE-EVAL**: Run evaluations again
6. **DECIDE**: Auto-commit if improvement ≥ +2%, revert if regression > 5%

### Example Output

```
Iteration 1/5
  Baseline: L5=FAIL (teaching)
  Hypothesis: Add structured example templates to teaching prompts
  Applying improvement...
  Re-eval: L5=PASS ✓ (+8.3% overall improvement)
  Decision: COMMIT (net gain +8.3%, no regression)

Iteration 2/5
  All levels passing. No further improvements needed.
  Final score: 12/12 (100%)
```

---

## Troubleshooting

### Agent fails to start

- Check that the SDK is installed and API keys are configured
- Verify the agent directory contains `main.py` and `agent_config.json`
- Check logs in `goal_agents/<name>/logs/`

### Low evaluation scores

- Review the goal prompt for clarity and specificity
- Ensure constraints are realistic
- Try a different SDK (Claude tends to be better for code-heavy tasks)

### Memory issues

- Verify memory is enabled: `--enable-memory`
- Check that the graph database is accessible
- Look for memory errors in agent logs

### Generation fails

- Validate the prompt file has all required sections
- Check that the skills directory exists
- Try with `--verbose` for detailed error output

---

## Related Documentation

- [Goal-Seeking Agents](../concepts/goal-seeking-agents.md) — concept overview
- [Goal Agent Generator](../concepts/agent-generator.md) — generation pipeline
- [Example Goal Prompts](../reference/goal-agent-example-prompt.md) — prompt templates
- [Evaluation Framework](../concepts/eval-framework.md) — evaluation system
- [Benchmarking](../reference/benchmarking.md) — performance measurement
