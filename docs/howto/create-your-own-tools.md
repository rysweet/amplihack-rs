# Create Your Own Tools

**Type**: How-To (Task-Oriented)

How to create new AI-powered tools with amplihack using metacognitive recipes.
You describe **how the AI should think** — amplihack builds the solution.

## Overview

Tool creation follows 5 steps:

1. **Identify a problem** — pick a task that is repetitive, complex, or time-consuming
2. **Outline a metacognitive recipe** — describe the step-by-step thinking process
3. **Use amplihack to build it** — launch creation with `/dev`
4. **Refine and iterate** — test, give feedback, improve
5. **Integrate** — use the tool, combine with others, contribute improvements

## Step 1: Identify the Problem

Choose a task you can describe clearly with a **concrete goal** and
**measurable success criteria**. Examples:

- Codebase analysis (find patterns across files)
- Documentation generation (create docs from code)
- Test coverage enhancement (identify untested paths)
- Security audit (scan for common vulnerabilities)

## Step 2: Formulate a Recipe

Write down the approach an expert would take. Focus on **how to think**,
not just what to do:

### Break Into Steps

Divide the problem into logical phases. Each step should be manageable:

1. Analyze code structure
2. Extract function signatures and docstrings
3. Generate API reference
4. Create usage examples
5. Review for completeness

!!! tip "Keep Steps Focused"
    Avoid making one agent handle everything. Smaller, focused steps
    improve reliability.

### Add Checkpoints

Build in review points: should the AI summarize findings before proceeding?
Should it pause for user confirmation on ambiguous results?

### Plan for Errors

Include fallback plans:

- "If analysis is incomplete, refine and retry"
- "If no examples can be generated, explain why"

## Step 3: Build with amplihack

Launch tool creation:

```
/dev I want to create a tool called "API Documentation Generator".

Goal: Analyze Python API code and generate comprehensive documentation.

Steps:
1. Scan directory for Python files with API endpoints
2. Extract function signatures, docstrings, type hints
3. Identify request/response models
4. Generate markdown documentation with examples
5. Validate all public endpoints are documented
6. Offer draft for review and incorporate feedback
```

amplihack will:

- Plan the architecture (architect agent)
- Create code modules (builder agent)
- Generate tests (tester agent)
- Review for quality (reviewer agent)

## Step 4: Refine and Iterate

After initial generation:

1. **Test the tool** on real data
2. **Provide feedback** — describe what worked and what didn't
3. **Iterate** — run `/dev` again with refinements
4. **Validate** — ensure the tool handles edge cases

## Step 5: Integrate

### Place the Tool

| Maturity      | Location                          |
| ------------- | --------------------------------- |
| Experimental  | `amplifier-bundle/ai_working/`    |
| Production    | `amplifier-bundle/scenarios/`     |

### Graduation Criteria

Move from experimental to production when:

- Proven value (2-3 successful uses)
- Complete documentation
- Comprehensive test coverage
- Stability (no breaking changes for 1+ week)

## Example: Code Quality Analyzer

```
/dev Create a "Code Quality Analyzer" tool that:

1. Scans Rust source files for complexity metrics
2. Identifies functions over 50 lines
3. Flags deep nesting (>3 levels)
4. Reports unused dependencies
5. Generates a prioritized list of improvements
6. Suggests specific refactoring for top 3 issues
```

## Related

- [Developing amplihack](../howto/develop-amplihack.md) — local development setup
- [Philosophy](../concepts/philosophy.md) — design principles guiding tool creation
- [Create Custom Agent](../howto/create-custom-agent.md) — agent creation guide
