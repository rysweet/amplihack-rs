# Agentic Step Patterns

When to use `bash`, `agent`, and `recipe` step types in YAML recipes,
and how to write effective agent prompts.

## Contents

- [Decision rule](#decision-rule)
- [Step types](#step-types)
- [When to use each type](#when-to-use-each-type)
- [Prompt writing guide](#prompt-writing-guide)
- [Anti-patterns](#anti-patterns)
- [Example recipe sequence](#example-recipe-sequence)

---

## Decision rule

**Default to `agent` steps.** Use `bash` only for deterministic operations
(git commands, file checks, API calls with known output). Use `recipe` to
compose larger workflows from smaller ones.

The reasoning: agent steps handle ambiguity, adapt to unexpected input, and
produce richer output. Bash steps are brittle when the output format or
error conditions vary.

## Step types

| Type | Executor | Output | Timeout default |
|---|---|---|---|
| `bash` | Shell subprocess | stdout captured to `output` key | 120s |
| `agent` | Claude/Copilot session | Full response captured to `output` key | `default_step_timeout` (recipe-level) |
| `recipe` | Nested recipe-runner-rs invocation | Final context from sub-recipe | Inherits from sub-recipe |

## When to use each type

### `bash` — Deterministic, verifiable operations

Use when:
- Running `git` commands (branch, commit, push, status)
- Checking file existence or reading structured output
- Calling APIs with predictable JSON responses (`gh issue view`)
- Setting up environment (mkdir, export, install)

```yaml
- id: "create-branch"
  type: "bash"
  command: |
    cd "$REPO_PATH"
    git checkout -b "feat/$BRANCH_NAME" 2>/dev/null || git checkout "feat/$BRANCH_NAME"
  output: "branch_result"
```

### `agent` — Reasoning, analysis, generation

Use when:
- Analyzing requirements or code
- Generating code, tests, or documentation
- Making design decisions
- Evaluating quality or correctness
- Anything requiring natural language understanding

```yaml
- id: "design-solution"
  type: "agent"
  timeout_seconds: 300
  prompt: |
    Design an implementation for the following requirement.

    **Task**: {{task_description}}
    **Requirements**: {{final_requirements}}
    **Constraints**: Rust codebase, must maintain backward compatibility.

    Output a concrete file-by-file implementation plan.
  output: "design"
```

### `recipe` — Workflow composition

Use when:
- A step is itself a multi-step workflow (e.g., running `default-workflow`
  from within `smart-orchestrator`)
- Reusing an existing recipe as a building block
- Isolating a complex sub-task with its own context

```yaml
- id: "run-default-workflow"
  type: "recipe"
  recipe: "default-workflow"
  context:
    task_description: "{{task_description}}"
    repo_path: "{{repo_path}}"
```

## Prompt writing guide

Agent steps live or die by their prompts. Follow these rules:

### 1. State the goal, not the process

```yaml
# Good: what to achieve
prompt: "Identify all public functions in {{file_path}} that lack test coverage."

# Bad: how to do it
prompt: "Read the file, then grep for fn, then check if tests exist..."
```

### 2. Provide full context via interpolation

Every `{{variable}}` referenced must be a context key populated by a prior
step. Never assume the agent remembers prior steps — each agent session
starts fresh.

### 3. Specify output format when downstream steps parse it

```yaml
prompt: |
    Classify this task.
    Respond with EXACTLY one line: `Development` or `Investigation` or `Q&A`.
output: "task_type"
```

### 4. Set appropriate timeouts

Agent steps that spawn full Claude sessions can exceed the default timeout.
The `smart-orchestrator.yaml` sets `default_step_timeout: 300` (5 minutes)
for this reason.

## Anti-patterns

| Anti-pattern | Problem | Fix |
|---|---|---|
| Bash step that parses natural language | Fragile regex on free-form text | Use an agent step |
| Agent step for `git checkout` | Wastes tokens on deterministic work | Use a bash step |
| Agent prompt without context vars | Agent has no information to work with | Interpolate `{{variables}}` |
| Nested recipe without forwarding context | Sub-recipe runs with empty context | Pass required context explicitly |
| No timeout on agent step | Hangs indefinitely on slow sessions | Set `timeout_seconds` |

## Example recipe sequence

From `default-workflow.yaml`, a typical sequence mixing all three types:

```yaml
steps:
  # 1. Bash: create GitHub issue (deterministic API call)
  - id: "create-issue"
    type: "bash"
    command: "gh issue create --title '...' --body '...'"
    output: "issue_url"

  # 2. Agent: design the solution (reasoning required)
  - id: "design"
    type: "agent"
    prompt: "Design implementation for: {{task_description}}"
    output: "design_spec"

  # 3. Agent: implement the code (generation required)
  - id: "implement"
    type: "agent"
    prompt: "Implement the design: {{design_spec}}"
    output: "implementation"

  # 4. Bash: run tests (deterministic command)
  - id: "test"
    type: "bash"
    command: "cargo test 2>&1"
    output: "test_results"

  # 5. Agent: evaluate results (judgment required)
  - id: "review"
    type: "agent"
    prompt: "Review test results: {{test_results}}"
    output: "review"
```

## Related

- [Recipe Execution Flow](./recipe-execution-flow.md) — How the runner processes steps
- [amplihack recipe](../reference/recipe-command.md) — CLI reference for recipe commands
- [Recipe Runner Architecture](./recipe-runner-architecture.md) — Why the runner is external
