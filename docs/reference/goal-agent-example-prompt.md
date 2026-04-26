# Example Goal Prompt

A template for creating goal-seeking agent prompts with the `amplihack new`
command.

## Template

```markdown
# Goal: <Primary objective in one sentence>

<Detailed description of what the agent should accomplish>

## Objective
<Specific, measurable objective>

## Domain
<Domain area: software-engineering, security, documentation, etc.>

## Constraints
- <Time limits>
- <Resource restrictions>
- <Scope boundaries>

## Success Criteria
- <Measurable outcome 1>
- <Measurable outcome 2>
- <Measurable outcome 3>

## Context
<Background information, domain knowledge, related systems>

## Technical Requirements
- <Specific tool or API integrations>
- <Language or framework requirements>
- <Output format specifications>
```

## Example: Code Review Agent

```markdown
# Goal: Automate Code Review Process

Create an autonomous agent that reviews pull requests and provides feedback.

## Objective

Build a system that:

- Analyzes code changes in pull requests
- Detects common issues and anti-patterns
- Generates constructive feedback
- Posts review comments automatically

## Domain

Automation and code quality

## Constraints

- Must complete review within 15 minutes
- Should not modify code directly
- Must respect existing code style
- Cannot access production systems

## Success Criteria

- All pull requests reviewed within SLA
- At least 80% of common issues detected
- Review comments are actionable and helpful
- No false positives on style violations
- Team satisfaction with review quality

## Technical Requirements

- Integrate with GitHub API
- Support multiple programming languages
- Generate reports in markdown format
- Track review metrics over time

## Context

This agent will help reduce code review bottlenecks and improve code quality
by providing fast, consistent, automated feedback on common issues, allowing
human reviewers to focus on architecture and business logic.
```

## Usage

```bash
# Generate from prompt file
amplihack new --file code_review_goal.md --enable-memory --verbose

# With specific SDK
amplihack new --file code_review_goal.md --sdk claude --name pr-reviewer

# With multi-agent architecture
amplihack new --file code_review_goal.md --multi-agent --enable-spawning
```

## Tips for Effective Prompts

1. **Be specific**: "Review Rust code for unsafe blocks" > "Review code"
2. **Measurable criteria**: "Detect 80% of issues" > "Find bugs"
3. **Clear constraints**: List what the agent should NOT do
4. **Context matters**: Explain why the agent exists and how it fits in
5. **Domain specification**: Helps the generator select appropriate skills

## Related Documentation

- [Goal-Seeking Agents](../concepts/goal-seeking-agents.md) — concept overview
- [Goal-Seeking Agent Tutorial](../howto/goal-seeking-agent-tutorial.md) — step-by-step guide
- [Goal Agent Generator](../concepts/agent-generator.md) — generation pipeline
