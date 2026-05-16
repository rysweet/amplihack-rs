# Integration Guide: goal_agent_generator Module

This guide explains how to integrate the `goal_agent_generator` module into your projects, workflows, and existing agents.

## Table of Contents

1. [Installation](#installation)
2. [Basic Usage](#basic-usage)
3. [API Reference](#api-reference)
4. [CLI Reference](#cli-reference)
5. [Integration Patterns](#integration-patterns)
6. [Advanced Customization](#advanced-customization)
7. [Troubleshooting](#troubleshooting)

## Installation

### Prerequisites

```bash
# Rust toolchain
rustc --version  # Stable Rust required

# amplihack framework
cargo install amplihack
```

### Verify Installation

```bash
amplihack --version
amplihack goal-agent-generator --help
```

## Basic Usage

### Quickest Start: CLI

```bash
# Create agent from inline prompt
amplihack goal-agent-generator create \
  --inline "Automate CI failure diagnosis and fix iteration" \
  --output .claude/agents/goal-driven/ci-fixer

# Execute agent
amplihack goal-agent-generator execute \
  --agent-path .claude/agents/goal-driven/ci-fixer \
  --auto-mode \
  --max-turns 10
```

### CLI Usage: 5-Step Process

```bash
# Step 1: Define goal (natural language) in a prompt file
cat > my-goal.md << 'EOF'
Automate data pipeline:
- Collect from multiple sources
- Transform to common schema
- Validate quality
- Publish to warehouse
EOF

# Step 2: Analyze goal
amplihack goal-agent-generator analyze --prompt-file my-goal.md

# Step 3: Generate execution plan
amplihack goal-agent-generator plan --prompt-file my-goal.md --output plan.json

# Step 4: Synthesize skills
amplihack goal-agent-generator synthesize --plan-file plan.json --output ./skills

# Step 5: Assemble and package agent
amplihack goal-agent-generator create \
  --prompt-file my-goal.md \
  --bundle-name "data-pipeline-agent" \
  --output .claude/agents/goal-driven/data-pipeline-agent

# Output:
# Agent created: data-pipeline-agent
# Estimated duration: shown in plan.json
```

## API Reference

### PromptAnalyzer

Extracts structured goal definitions from natural language.

**Methods**:

```python
class PromptAnalyzer:
    def analyze(self, prompt_path: Path) -> GoalDefinition:
        """Analyze prompt from file"""

    def analyze_text(self, prompt: str) -> GoalDefinition:
        """Analyze prompt from text"""
```

**GoalDefinition Fields**:

```python
@dataclass
class GoalDefinition:
    raw_prompt: str              # Original prompt text
    goal: str                    # Extracted primary goal
    domain: str                  # Classified domain
    constraints: list[str]       # Technical/operational constraints
    success_criteria: list[str]  # Measurable success criteria
    complexity: str              # "simple", "moderate", "complex"
    context: dict                # Additional metadata
```

**Domain Classification**:

- `data-processing`: Data transformation, analysis, ETL
- `security-analysis`: Vulnerability scanning, audits
- `automation`: Workflow automation, scheduling
- `testing`: Test generation, validation
- `deployment`: Release, publishing
- `monitoring`: Observability, alerting
- `integration`: API connections
- `reporting`: Dashboards, metrics

**Example**:

```python
analyzer = PromptAnalyzer()
goal_def = analyzer.analyze_text("Audit AKS cluster security and compliance")

print(goal_def.goal)        # "Audit AKS cluster security and compliance"
print(goal_def.domain)      # "security-analysis"
print(goal_def.complexity)  # "moderate"
```

### ObjectivePlanner

Generates multi-phase execution plans with dependencies.

**Methods**:

```python
class ObjectivePlanner:
    def generate_plan(self, goal_definition: GoalDefinition) -> ExecutionPlan:
        """Generate execution plan from goal"""
```

**ExecutionPlan Fields**:

```python
@dataclass
class ExecutionPlan:
    goal_id: uuid.UUID
    phases: list[PlanPhase]
    total_estimated_duration: str
    required_skills: list[str]
    parallel_opportunities: list[list[str]]
    risk_factors: list[str]
```

**PlanPhase Fields**:

```python
@dataclass
class PlanPhase:
    name: str
    description: str
    required_capabilities: list[str]
    estimated_duration: str
    dependencies: list[str]       # Names of prerequisite phases
    parallel_safe: bool          # Can execute in parallel
    success_indicators: list[str]
```

**Example**:

```python
planner = ObjectivePlanner()
plan = planner.generate_plan(goal_def)

print(f"Phases: {len(plan.phases)}")
print(f"Duration: {plan.total_estimated_duration}")

for phase in plan.phases:
    print(f"- {phase.name}: {phase.estimated_duration}")
    print(f"  Dependencies: {phase.dependencies}")
    print(f"  Parallel: {phase.parallel_safe}")
```

### SkillSynthesizer

Maps execution capabilities to skills.

**Methods**:

```python
class SkillSynthesizer:
    def synthesize(self, execution_plan: ExecutionPlan) -> list[SkillDefinition]:
        """Synthesize skills from execution plan"""
```

**SkillDefinition Fields**:

```python
@dataclass
class SkillDefinition:
    name: str
    description: str
    capabilities: list[str]
    implementation_type: str  # "native" or "delegated"
    delegation_target: str    # Agent to delegate to (if delegated)
```

**Example**:

```python
synthesizer = SkillSynthesizer()
skills = synthesizer.synthesize(plan)

for skill in skills:
    print(f"Skill: {skill.name}")
    print(f"  Capabilities: {skill.capabilities}")
    if skill.implementation_type == "delegated":
        print(f"  Delegates to: {skill.delegation_target}")
```

### AgentAssembler

Combines components into executable agent bundle.

**Methods**:

```python
class AgentAssembler:
    def assemble(
        self,
        goal_definition: GoalDefinition,
        execution_plan: ExecutionPlan,
        skills: list[SkillDefinition],
        bundle_name: str = None  # Auto-generated if not provided
    ) -> GoalAgentBundle:
        """Assemble complete agent bundle"""
```

**GoalAgentBundle Fields**:

```python
@dataclass
class GoalAgentBundle:
    id: uuid.UUID
    name: str
    version: str
    goal_definition: GoalDefinition
    execution_plan: ExecutionPlan
    skills: list[SkillDefinition]
    auto_mode_config: dict
    metadata: dict
    status: str
```

**Example**:

```python
assembler = AgentAssembler()
bundle = assembler.assemble(
    goal_definition=goal_def,
    execution_plan=plan,
    skills=skills,
    bundle_name="my-custom-agent"
)

print(f"Agent: {bundle.name}")
print(f"Max turns: {bundle.auto_mode_config['max_turns']}")
print(f"Status: {bundle.status}")
```

### GoalAgentPackager

Packages agent bundle for deployment.

**Methods**:

```python
class GoalAgentPackager:
    def package(
        self,
        bundle: GoalAgentBundle,
        output_dir: Path
    ) -> None:
        """Package bundle to directory"""
```

**Output Structure**:

```
output_dir/
├── agent.md           # Agent definition
├── prompt.md          # Initial execution prompt
├── metadata.json      # Bundle metadata
├── plan.yaml          # Execution plan
└── skills.yaml        # Required skills
```

**Example**:

```python
packager = GoalAgentPackager()
packager.package(
    bundle=bundle,
    output_dir=Path(".claude/agents/goal-driven/my-agent")
)
```

## CLI Reference

### create: Generate Agent

```bash
amplihack goal-agent-generator create [OPTIONS]

Options:
  --prompt PATH          Path to prompt.md file
  --inline TEXT          Inline prompt text
  --output PATH          Output directory for agent
  --bundle-name TEXT     Custom agent name (auto-generated if omitted)
  --help                 Show help message
```

**Examples**:

```bash
# From file
amplihack goal-agent-generator create \
  --prompt ./prompts/data-pipeline.md \
  --output .claude/agents/goal-driven/data-pipeline

# Inline prompt
amplihack goal-agent-generator create \
  --inline "Automate security audits" \
  --output .claude/agents/goal-driven/security-auditor \
  --bundle-name security-auditor
```

### execute: Run Agent

```bash
amplihack goal-agent-generator execute [OPTIONS]

Options:
  --agent-path PATH      Path to agent directory
  --auto-mode            Run in auto-mode (autonomous)
  --max-turns INT        Maximum auto-mode turns (default: from bundle)
  --dry-run              Show plan without executing
  --resume-from-phase INT  Resume from specific phase
  --help                 Show help message
```

**Examples**:

```bash
# Basic execution
amplihack goal-agent-generator execute \
  --agent-path .claude/agents/goal-driven/data-pipeline \
  --auto-mode

# Custom max turns
amplihack goal-agent-generator execute \
  --agent-path .claude/agents/goal-driven/ci-fixer \
  --auto-mode \
  --max-turns 15

# Dry run (show plan)
amplihack goal-agent-generator execute \
  --agent-path .claude/agents/goal-driven/security-auditor \
  --dry-run

# Resume from phase 3
amplihack goal-agent-generator execute \
  --agent-path .claude/agents/goal-driven/release-workflow \
  --resume-from-phase 3
```

### list: List Agents

```bash
amplihack goal-agent-generator list [OPTIONS]

Options:
  --directory PATH       Directory to search (default: .claude/agents/goal-driven)
  --format [table|json]  Output format
  --help                 Show help message
```

**Example**:

```bash
amplihack goal-agent-generator list

# Output:
# Name                  Domain            Phases  Complexity  Status
# data-pipeline-agent   data-processing   4       moderate    ready
# ci-fixer              automation        5       moderate    ready
# security-auditor      security-analysis 4       simple      ready
```

## Integration Patterns

### Pattern 1: Integrate with Existing Workflows

```bash
# In your workflow orchestration, dynamically create a goal agent from a task description
amplihack goal-agent-generator analyze \
  --prompt "Your task description here"

amplihack goal-agent-generator plan \
  --prompt-file task-goal.md \
  --output plan.json

# Use plan for orchestration — execute each phase
amplihack goal-agent-generator execute \
  --agent-path .claude/agents/goal-driven/my-task-agent \
  --auto-mode
```

### Pattern 2: Integrate with CI/CD

```yaml
# .github/workflows/deploy.yml
name: Deploy with Goal Agent

on:
  push:
    branches: [main]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install Rust toolchain
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Install amplihack
        run: cargo install amplihack

      - name: Execute deployment agent
        run: |
          amplihack goal-agent-generator execute \
            --agent-path .claude/agents/goal-driven/deployment-agent \
            --auto-mode \
            --max-turns 10
```

### Pattern 3: Integrate with Existing Agents

```bash
# In existing agent workflows, delegate complex tasks to goal-seeking agent
# First analyze if the task warrants a goal-seeking approach
amplihack goal-agent-generator analyze \
  --prompt "Your complex task description here" \
  --output analysis.json

# If complexity is "complex", use goal-seeking agent
amplihack goal-agent-generator create \
  --prompt "Your complex task description here" \
  --output .claude/agents/goal-driven/complex-task

amplihack goal-agent-generator execute \
  --agent-path .claude/agents/goal-driven/complex-task \
  --auto-mode
```

### Pattern 4: Custom Phase Templates

```bash
# Custom phase templates can be applied via config overlays
# Create a domain-specific config file:
cat > ml-config.json << 'EOF'
{
  "domain_overrides": {
    "ml-training": [
      {"name": "Data Preparation", "description": "Prepare training data", "capabilities": ["data-loading", "cleaning"]},
      {"name": "Model Training", "description": "Train ML model", "capabilities": ["training", "validation"]},
      {"name": "Model Evaluation", "description": "Evaluate model performance", "capabilities": ["testing", "metrics"]},
      {"name": "Model Deployment", "description": "Deploy model to production", "capabilities": ["deployment", "monitoring"]}
    ]
  }
}
EOF

# Use the custom config with plan generation
amplihack goal-agent-generator plan \
  --prompt-file ml-goal.md \
  --config ml-config.json \
  --output plan.json
```

## Advanced Customization

### Custom Goal Analysis

```bash
# Custom domain keywords can be configured via a config overlay
cat > custom-analyzer-config.json << 'EOF'
{
  "domain_keywords": {
    "ml-training": ["train", "model", "machine learning", "neural", "dataset"],
    "iot-processing": ["sensor", "device", "telemetry", "iot", "edge"]
  },
  "keyword_overrides": {
    "tensorflow": "ml-training",
    "pytorch": "ml-training"
  }
}
EOF

# Analyze with custom domain classification
amplihack goal-agent-generator analyze \
  --prompt-file my-goal.md \
  --config custom-analyzer-config.json
```

### Custom Skill Mapping

```bash
# Custom skill mappings can be configured via a config overlay
cat > custom-skills-config.json << 'EOF'
{
  "skill_mapping": {
    "ml-*": "ml-engineer",
    "data-viz-*": "visualization-specialist",
    "iot-*": "iot-processor"
  }
}
EOF

# Synthesize skills with custom mapping
amplihack goal-agent-generator synthesize \
  --plan-file plan.json \
  --config custom-skills-config.json \
  --output ./skills
```
```

## Troubleshooting

### Issue 1: Domain Misclassified

**Problem**: Goal classified as wrong domain

**Solution**: Use explicit domain keywords

```python
# Add domain-specific keywords
goal_text = """
Deploy Kubernetes application to production.
Domain: deployment
"""

# Or customize PromptAnalyzer
analyzer = CustomPromptAnalyzer()  # See Custom Goal Analysis above
```

### Issue 2: Too Many/Few Phases

**Problem**: Plan has too many or too few phases

**Solution**: Adjust goal complexity or customize planner

```python
# Simplify goal (fewer phases)
goal_text = "Collect and publish data"  # 2 phases

# vs more detailed goal (more phases)
goal_text = "Collect, validate, transform, quality-check, and publish data"  # 5 phases
```

### Issue 3: Wrong Skills Generated

**Problem**: Skills don't match available agents

**Solution**: Customize skill synthesizer or use explicit capability names

```python
# Use capability names that match your agents
phase.required_capabilities = [
    "azure-kubernetes-operations",  # Matches azure-kubernetes-expert agent
    "ci-diagnostics",               # Matches ci-diagnostic-workflow agent
]
```

### Issue 4: Execution Timeout

**Problem**: Agent exceeds max_turns

**Solution**: Increase max_turns or simplify goal

```bash
# Increase max turns
amplihack goal-agent-generator execute \
  --agent-path .claude/agents/goal-driven/my-agent \
  --auto-mode \
  --max-turns 20  # Default is based on complexity

# Or simplify goal into smaller goals
```

### Issue 5: Phase Failures

**Problem**: Agent fails at specific phase

**Solution**: Resume from failed phase after fixing issue

```bash
# Fix issue manually, then resume
amplihack goal-agent-generator execute \
  --agent-path .claude/agents/goal-driven/my-agent \
  --resume-from-phase 3  # Resume from phase 3
```

## Best Practices

1. **Start Simple**: Begin with simple goals, add complexity incrementally
2. **Clear Success Criteria**: Define measurable success criteria
3. **Specify Constraints**: Include resource limits, time limits, safety requirements
4. **Test Dry Runs**: Use `--dry-run` to review plan before execution
5. **Monitor Execution**: Watch progress, be ready to intervene
6. **Iterate**: Refine goal prompts based on results
7. **Document Learnings**: Update DISCOVERIES.md with insights

## Getting Help

**Module Issues**:

- Check logs: `~/.amplihack/.claude/runtime/logs/<session_id>/`
- Review agent bundle: `<agent-path>/metadata.json`

**Integration Issues**:

- Review this integration guide
- Check API reference above
- Study examples in `examples/` directory

**Design Issues**:

- Use decision framework (SKILL.md Section 2)
- Review architecture pattern (SKILL.md Section 3)
- Consult design checklist (SKILL.md Section 6)

**Community**:

- Contribute improvements to amplihack repository
- Share your goal-seeking agent patterns
- Document reusable skill templates

---

**Remember**: The `goal_agent_generator` module is designed for flexibility. Start with defaults, customize as needed, and share your learnings with the community.
