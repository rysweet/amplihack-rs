# amplihack

**Agentic coding framework for Claude Code, Microsoft Amplifier, GitHub Copilot CLI, and Codex that uses specialized AI agents to accelerate software development through intelligent automation and collaborative problem-solving.**

## What is amplihack?

amplihack is a development framework for popular coding agent systems (Claude Code, Microsoft Amplifier, GitHub Copilot CLI, and Codex) that leverages multiple specialized AI agents working together to handle complex software development tasks. It combines ruthless simplicity with powerful capabilities to make AI-assisted development more effective and maintainable.

## Quick Navigation

**New to amplihack?** Start here:

- [Get Started](#-get-started) - Installation and first steps
- [Core Concepts](#-core-concepts) - Philosophy and principles
- [Amplihack Tutorial](tutorials/amplihack-tutorial.md) - Comprehensive 60-90 minute tutorial

**Looking for something specific?**

- [Code Atlas](atlas/index.md) - Architecture diagrams, dependency maps, and bug hunt results
- [Commands & Operations](#%EF%B8%8F-commands--operations) - Execute complex tasks
- [Workflows](#-workflows) - Structured development processes
- [Agents & Tools](#-agents--tools) - Specialized AI capabilities
- [Troubleshooting](#-troubleshooting--discoveries) - Fix common issues

---

## 🚀 Get Started

Everything you need to install and configure amplihack.

### Prerequisites

- Rust 1.88+ / cargo
- Node.js 18+ / npm
- git
- cmake and a C/C++ toolchain for LadybugDB

### Choose Your Tool

amplihack works with multiple agentic coding tools. Choose the one that fits your workflow:

```sh
# Install, then launch with Claude Code (default)
npx --yes --package=git+https://github.com/rysweet/amplihack-rs.git -- amplihack install
amplihack claude

# Or launch with Microsoft Amplifier
amplihack amplifier

# Or launch with GitHub Copilot CLI
amplihack copilot
```

**Tool Compatibility Matrix:**

| Feature             | Claude Code | Amplifier | Copilot CLI | Codex      |
| ------------------- | ----------- | --------- | ----------- | ---------- |
| Plugin Architecture | ✅ Yes      | ❌ No     | ❌ No       | ❌ No      |
| Per-Project Staging | ✅ Yes      | ✅ Yes    | ✅ Yes      | ✅ Yes     |
| All Agents (42)     | ✅ Yes      | ✅ Yes    | ✅ Yes      | ⚠️ Limited |
| All Skills (120)    | ✅ Yes      | ✅ Yes    | ✅ Yes      | ⚠️ Limited |
| All Commands (31)   | ✅ Yes      | ✅ Yes    | ✅ Yes      | ⚠️ Limited |
| Workflows           | ✅ All      | ✅ All    | ✅ All      | ⚠️ Limited |
| Auto Mode           | ✅ Yes      | ✅ Yes    | ✅ Yes      | ⚠️ Limited |

See [amplihack-rs parity](amplihack-rs-parity.md) for subprocess
prompt delivery support across Claude Code, Amplifier, Copilot CLI, and Codex.

**New to amplihack?** After launching, try the interactive tutorial:

```
Task(subagent_type='guide', prompt='I am new to amplihack. Teach me the basics.')
```

The guide agent will walk you through workflows, prompting strategies, and hands-on exercises (60-90 minutes).

### Plugin Architecture ⭐ NEW

Centralized plugin system that works across all your projects:

- [Plugin Installation Guide](plugin/INSTALLATION.md) - Install amplihack as a global plugin (Claude Code only)
- [Plugin Architecture Overview](plugin/ARCHITECTURE.md) - How the plugin system works
- [Migration Guide](plugin/MIGRATION.md) - Migrate from per-project to plugin mode
- [CLI Reference](plugin/CLI_REFERENCE.md) - Complete command-line reference

**Note**: Plugin architecture is **Claude Code only**. Microsoft Amplifier, GitHub Copilot CLI, and Codex use per-project `~/.amplihack/.claude/` staging instead.

### Installation

- [Prerequisites](PREREQUISITES.md) - System requirements and dependencies
- [Interactive Installation](INTERACTIVE_INSTALLATION.md) - Step-by-step setup wizard
- [Quick Start](../README.md) - Basic usage and first commands
- [PATH Conflict Tutorial](tutorials/repair-install-update-path-conflicts.md) - Learn why stale system binaries can shadow current user-local installs
- [Repair Install/Update PATH Conflicts](howto/repair-install-update-path-conflicts.md) - Fix stale `/usr/local/bin` binaries that shadow current `~/.local/bin` installs
- [Repair a Stale Framework Bundle](howto/repair-stale-framework-bundle.md) - Fix installs where the binary is current but smart-orchestrator recipes are stale

### Configuration

#### Tool-Specific Setup

**Claude Code (Default)**

- Requires: `$ANTHROPIC_API_KEY` environment variable for Anthropic models
- Plugin mode: Install globally with [Plugin Installation Guide](plugin/INSTALLATION.md)
- Per-project mode: Copy `~/.amplihack/.claude/` directory to your project
- Azure OpenAI: Configure via environment variables

**Microsoft Amplifier**

```sh
amplihack amplifier
```

Amplifier walks you through model configuration on first startup. Supports all Amplifier-compatible models including Claude, GPT-4, and local models.

**GitHub Copilot CLI**

```sh
amplihack copilot
```

- Uses GitHub Copilot models (switch with `/model` command)
- Adaptive hooks enable preference injection and context loading
- All 42 bundled agents available via `--agent <name>` flag
- See [GitHub Copilot CLI](COPILOT_CLI.md) for complete guide
- See [How to Use amplihack with a Non-Claude Agent](howto/use-non-claude-agent.md) for `AMPLIHACK_AGENT_BINARY` propagation and nested Copilot compatibility details
- Follow the [Copilot parity control-plane tutorial](tutorials/copilot-parity-control-plane.md) for step-by-step setup and validation
- Use [How to Configure the Copilot Parity Control Plane](howto/configure-copilot-parity-control-plane.md) to pin the runner, pick the hook engine, and verify XPIA behavior
- Read [Understanding the Copilot Parity Control Plane](concepts/copilot-parity-control-plane.md) for the architecture and trade-offs
- Use the [Copilot Parity Control Plane Reference](reference/copilot-parity-control-plane.md) for precedence, hook contracts, and environment variables

**Codex**

- Limited support via per-project `~/.amplihack/.claude/` staging
- Most features work but may require adaptation
- Tested primarily with Claude models

#### General Configuration

- [Profile Management](PROFILE_MANAGEMENT.md) - Multiple environment configurations
- [Hook Configuration](HOOK_CONFIGURATION_GUIDE.md) - Customize framework behavior
- [Artifact Guard](artifact-guard.md) - Guard for broad staging, pre-commit, and workflow publication artifacts
- [Configure Workflow Runtime Isolation](howto/configure-workflow-runtime-isolation.md) - Keep generated workflow runtime output outside commit worktrees
- [Memory Configuration Consent](features/memory-consent-prompt.md) - Intelligent memory settings with timeout protection
- [Verify .claude/ Staging](howto/verify-claude-staging.md) - Check that framework files are properly staged
- [Verify Runtime Asset Resolution](howto/verify-runtime-asset-resolution.md) - Check helper, session, hooks, and multitask asset parity
- [Verify Framework Injection](howto/verify-framework-injection.md) - Check that AMPLIHACK.md injection is working
- [Enable Blarify Code Indexing](howto/enable-blarify.md) - Opt-in code graph indexing with env var, non-interactive mode, and staleness detection

### Deployment

- [Release Installation](howto/first-install.md) - Install from published binaries or source
- [Install Manifest](reference/install-manifest.md) - Understand installed files and uninstall state
- [Install/Update PATH Conflict Reference](reference/install-update-path-conflicts.md) - Target selection, PATH analysis, and output regression guarantees
- [Framework Bundle Compatibility](reference/framework-bundle-compatibility.md) - Smart-orchestrator compatibility contract and stale bundle repair behavior
- [Azure Integration](AZURE_INTEGRATION.md) - Deploy to Azure cloud

---

## 💡 Core Concepts

Understand the philosophy and architecture behind amplihack.

### Philosophy & Principles

- [Development Philosophy](PHILOSOPHY.md) - Ruthless simplicity and modular design
- [This Is The Way](THIS_IS_THE_WAY.md) - Best practices and patterns
- [Workspace Pattern](WORKSPACE_PATTERN.md) - Organize your development environment
- [Trust & Anti-Sycophancy](claude/context/TRUST.md) - Honest agent behavior

### Architecture

- [Project Overview](claude/context/PROJECT.md) - System architecture
- [Development Patterns](claude/context/PATTERNS.md) - Proven implementation patterns
- [Unified Staging Architecture](concepts/unified-staging-architecture.md) - How .claude/ staging works across all commands
- [Framework Injection Architecture](concepts/framework-injection-architecture.md) - How AMPLIHACK.md injection works
- [Unified Distributed Cognitive Memory](concepts/unified-distributed-cognitive-memory.md) - Planned architecture for deterministic cluster-wide memory retrieval
- [Goal-Seeking Agents](GOAL_SEEKING_AGENTS.md) - Overview of generated agents and the shared LearningAgent engine
- [LearningAgent Module Architecture](concepts/learning-agent-module-architecture.md) - How the refactored LearningAgent is split into focused modules
- [LearningAgent Refactor Tutorial](tutorials/learning-agent-refactor-tutorial.md) - Walk through learning, retrieval, and temporal answering in the split module design
- [Maintain the Refactored LearningAgent](howto/maintain-learning-agent-modules.md) - Contributor workflow for changing ingestion, retrieval, temporal logic, and synthesis
- [LearningAgent Module Reference](reference/learning-agent-module-reference.md) - Public API, configuration knobs, ownership map, and validation commands
- [How to Use Blarify Code Graph](howto/blarify-code-graph.md) - Enable, query, and configure
- [Enable Blarify Code Indexing](howto/enable-blarify.md) - `AMPLIHACK_ENABLE_BLARIFY`, non-interactive skip, staleness detection
- [Blarify Architecture](blarify_architecture.md) - Understanding the Blarify integration
- [Documentation Knowledge Graph](documentation_knowledge_graph.md) - How docs connect
- [amplihack-rs Parity Reference](amplihack-rs-parity.md) - Subprocess prompt delivery capabilities, configuration, diagnostics, and Rust API

### Key Features

- [Modular Design](#-modular-design-philosophy) - Self-contained modules (bricks & studs)
- [Zero-BS Implementation](#-zero-bs-implementation) - No stubs or placeholders
- [Specialized AI Agents](#-specialized-ai-agents) - Purpose-built for each task
- [Structured Workflows](#-structured-workflows) - Proven methodologies

---

## 📋 Workflows

Proven methodologies for consistent, high-quality results.

- [Workflow Commit Identity](workflows.md) - Planned safe author and committer
  attribution for Amplihack-created workflow commits

### Automatic Workflow Classification ⭐ NEW

**Mandatory at Session Start** (Issue #2353) - amplihack now automatically classifies your request and executes the appropriate workflow when you start a session:

- **4-Way Classification**: Q&A, Operations, Investigation, or Development
- **Recipe Runner Execution**: Code-enforced workflow steps with fail-fast behavior
- **Graceful Fallback**: Recipe Runner → Workflow Skills → Markdown
- **Explicit Command Bypass**: Commands like `/fix`, `/analyze` skip auto-classification

**Quick Reference**:
| Your Request | Classified As | Workflow | Steps |
|--------------|---------------|----------|-------|
| "What is..." | Q&A | Direct answer | 3 |
| "Clean up..." | Operations | Direct execution | 1 |
| "How does X work?" | Investigation | Deep exploration | 6 |
| "Add feature X" | Development | Full workflow | 23 |

**Implementation**: `src/amplihack/workflows/` - classifier, execution_tier_cascade, session_start modules

### Core Workflows

- [Default Workflow](claude/skills/default-workflow/SKILL.md) - Standard multi-step development process
- [Investigation Workflow](claude/workflow/INVESTIGATION_WORKFLOW.md) - Deep codebase analysis and understanding
- [PR Recovery Readiness](PR_RECOVERY_READINESS.md) - Entry point for existing-PR recovery docs and the canonical readiness contract
- [Document-Driven Development (DDD)](document_driven_development/README.md) - Documentation-first approach for large features

### DDD Deep Dive

Document-Driven Development is a systematic methodology where documentation comes first and acts as the specification.

- **Core Concepts**
  - [Overview](document_driven_development/README.md) - What is DDD and when to use it
  - [Core Concepts](document_driven_development/core_concepts/README.md) - File crawling, context poisoning, retcon writing
  - [File Crawling](document_driven_development/core_concepts/file_crawling.md) - Systematic file processing
  - [Context Poisoning](document_driven_development/core_concepts/context_poisoning.md) - Preventing inconsistencies
  - [Retcon Writing](document_driven_development/core_concepts/retcon_writing.md) - Present-tense documentation

- **Implementation Phases**
  - [Phase 0: Planning](document_driven_development/phases/00_planning_and_alignment.md) - Define scope and objectives
  - [Phase 1: Documentation](document_driven_development/phases/01_documentation_retcon.md) - Write the spec
  - [Phase 2: Approval](document_driven_development/phases/02_approval_gate.md) - Review and validate
  - [Phase 3: Code Planning](document_driven_development/phases/03_implementation_planning.md) - Implementation strategy
  - [Phase 4: Implementation](document_driven_development/phases/04_code_implementation.md) - Build it
  - [Phase 5: Testing](document_driven_development/phases/05_testing_and_verification.md) - Verify behavior
  - [Phase 6: Cleanup](document_driven_development/phases/06_cleanup_and_push.md) - Final touches

- **Reference**
  - [Reference Materials](document_driven_development/reference/README.md) - Practical resources
  - [Checklists](document_driven_development/reference/checklists.md) - Phase verification
  - [Tips for Success](document_driven_development/reference/tips_for_success.md) - Best practices
  - [Common Pitfalls](document_driven_development/reference/common_pitfalls.md) - What to avoid
  - [FAQ](document_driven_development/reference/faq.md) - Common questions

### Recipe Runner

Code-enforced workflow execution engine with declarative YAML recipes.

- [Recipe Runner Overview](recipes/README.md) - Architecture, YAML format, and creating custom recipes
- [UltraThink Recipe Runner Integration](recipes/RECIPE_RUNNER_ULTRATHINK_INTEGRATION.md) - How ultrathink uses Recipe Runner for code-enforced workflow execution
- [Recipe CLI Commands How-To](howto/recipe-cli-commands.md) - Task-oriented guide for using recipe commands
- [Dev-Orchestrator Routing Contract](reference/dev-orchestrator-routing.md) - Issue #749 target rules for Development and hybrid workstream routing
- [Verify Dev-Orchestrator Routing](howto/verify-dev-orchestrator-routing.md) - Check deterministic Development-to-default-workflow routing from the CLI
- [Correlate Recipe Runs with Logs](howto/correlate-recipe-runs.md) - Match terminal output, final JSON, child process IDs, and runner log paths by run ID
- [Recipe Executor Environment](reference/recipe-executor-environment.md) - Step-level variables plus subprocess environment contract for forced non-interactive recipe execution
- [Recipe Context Environment Export](reference/recipe-context-environment.md) - Export recipe context variables to bash steps (`$TASK_DESCRIPTION`, `$REPO_PATH`), uppercasing, reserved-name denylist, and precedence
- [Tutorial: Propagate Recipe Context to Bash Steps](tutorials/recipe-context-env-propagation.md) - Read context from the environment under `set -u`, including nested sub-recipes and skipped keys
- [Validate Recipe Subprocess and Hook Input Contracts](howto/validate-recipe-subprocess-hook-contract.md) - Validate recipe child env handling and hook JSON compatibility
- [Workflow Publish Import Validation](features/workflow-publish-import-validation.md) - Scoped publish import validation before commit/push
- [How to Configure Workflow Publish Import Validation](howto/configure-workflow-publish-import-validation.md) - Review the manifest, root-boundary, and scoped-validator behavior
- [Tutorial: Workflow Publish Import Validation](tutorials/workflow-publish-import-validation.md) - Walk through the scoped publish-validation flow
- [Workflow Publish Import Validation Reference](reference/workflow-publish-import-validation.md) - Manifest format, root-resolution rules, and `--files-from` semantics
- [Workflow Execution Guardrails](features/workflow-execution-guardrails.md) - Canonical execution roots, exact GitHub identity checks, and observer-only stall detection
- [How to Configure Workflow Execution Guardrails](howto/configure-workflow-execution-guardrails.md) - Supply `expected_gh_account`, inspect `execution_root`, and troubleshoot failures
- [Tutorial: Workflow Execution Guardrails](tutorials/workflow-execution-guardrails.md) - End-to-end walkthrough of the guarded workflow contract
- [Workflow Execution Guardrails Reference](reference/workflow-execution-guardrails.md) - Input fields, output schema, signals, and failure semantics
- [Workflow Runtime Isolation](features/workflow-runtime-isolation.md) - Runtime-root isolation and narrow cleanup for generated workflow artifacts
- [How to Configure Workflow Runtime Isolation](howto/configure-workflow-runtime-isolation.md) - Set `AMPLIHACK_RUNTIME_ROOT`, inspect runtime files, and troubleshoot guard failures
- [Tutorial: Workflow Runtime Isolation](tutorials/workflow-runtime-isolation.md) - Practice the runtime isolation and strict Artifact Guard contract
- [Workflow Runtime Artifacts Reference](reference/workflow-runtime-artifacts.md) - Environment variables, shell helper API, lifecycle hooks, and regression contract
- [Workflow Commit Identity Reference](reference/workflow-commit-identity.md) - Explicit author/committer identity contract for workflow-created commits
- [How to Configure Workflow Commit Identity](howto/configure-workflow-commit-identity.md) - Setup for `AMPLIHACK_GIT_*`, repo-local Git identity, and provider-safe fallbacks
- [Tutorial: Verify Workflow Commit Identity](tutorials/workflow-commit-identity.md) - Disposable-repository walkthrough for explicit, repo-local, GitHub, and Azure DevOps commit attribution
- [Default Workflow Step 13 Validation Reference](reference/default-workflow-step-13-validation.md) - Toolchain-aware outside-in local validation contract for Step 13
- [Workflow Terminal-State Reference](reference/workflow-terminal-state.md) - Target contract for development completion evidence, no-op states, failure semantics, and shell helper API
- [Default Workflow Agentic Finalization](reference/default-workflow-agentic-finalization.md) - Structured finalizer schema, terminal-state vocabulary, configuration, and examples for robust workflow closure
- [How to Diagnose Workflow Terminal-State Failures](howto/diagnose-workflow-terminal-state.md) - Investigate missing evidence after planning, analysis, design, or worktree-only runs
- [Tutorial: Workflow Terminal-State Closure](tutorials/workflow-terminal-state-closure.md) - Walk through completion evidence, planning-only failure, and rerun semantics
- [Workflow-Owned PR Recovery Readiness](features/pr-recovery-readiness.md) - Recover an existing PR through `default-workflow` with branch reuse, hook readiness, additive-copy readiness, and workflow-owned finalization
- [Existing Branch Finalization Runbook](features/post-v0977-finalization.md) - Finish an already-implemented branch through preservation, validation, PR publication, CI gating, and policy-ready merge
- [How to Recover an Existing PR with `default-workflow`](howto/recover-existing-pr-with-default-workflow.md) - Supply `pr_number`, `existing_branch`, and issue requirements without manually merging
- [Tutorial: Recover PR 579 Readiness](tutorials/pr-recovery-readiness.md) - Walk through hook and additive-copy readiness evidence for an interrupted PR recovery
- [PR Recovery Readiness Reference](reference/pr-recovery-readiness.md) - Context fields, readiness evidence schema, publish contract, and finalization states
- [Step 03 Host-Aware Tracking Idempotency](reference/recipe-step-03-idempotency.md) - GitHub issue, Azure Boards work-item, and local tracking reuse/create behavior
- [Workflow Issue Extraction Reference](reference/workflow-issue-extraction.md) - Three-tier issue-number resolution (direct URL → PR closing-refs → `#N` verify) in step 03b
- [Multi-Provider Workflow Reference](reference/multi-provider-workflow.md) - Provider detection, issue tracking, and PR routing for GitHub, AzDO, and local repos
- [How to Use the Workflow with Azure DevOps](howto/use-workflow-with-azure-devops.md) - Run default-workflow against an AzDO repository
- [Multi-Provider Workflow Architecture](concepts/multi-provider-workflow-architecture.md) - Design decisions: detect-once pattern, issue number contract, PR asymmetry
- [Run a Quality Audit](howto/run-quality-audit.md) - Invoke quality-audit-cycle recipe, target subdirectories, filter categories
- [CLI Reference](reference/cli.md) - Top-level `amplihack` command, `--version` flag, global environment variables
- [Recipe CLI Reference](reference/recipe-cli-reference.md) - Complete command-line documentation
- [Quality Audit Cycle Recipe](reference/quality-audit-cycle-recipe.md) - Context variables, step reference, bash step safety patterns
- [Token Sanitizer](reference/token-sanitizer.md) - Pattern ordering, audit labels, and custom patterns for secret redaction
- [RecipeResult](reference/recipe-result.md) - `RecipeResult` and `StepResult` dataclasses, `str()` format, JSON serialisation
- [AppendHandler](reference/append-handler.md) - `AppendResult` class, timestamp filename format, atomic file writes
- [Rust Runner Execution Reference](reference/rust-runner-execution.md) - Wrapper contract, stderr/stdout streams, JSONL events, environment variables, and security model
- [Rust Runner Execution Architecture](concepts/rust-runner-execution-architecture.md) - CLI/runner responsibility split, stream ownership, progress events, and additive JSON preservation
- [Recipe Runner Logging Reference](reference/recipe-runner-logging.md) - stderr progress, heartbeats, bounded snippets, JSONL events, and additive result fields
- [Recipe Run Correlation Reference](reference/recipe-run-correlation.md) - Stable run IDs, `AMPLIHACK_RECIPE_RUN_ID`, pointer event schema, and final result fields
- [Observe Recipe Progress Tutorial](tutorials/recipe-progress-transparency.md) - Watch live workflow progress, capture JSON, and debug failed steps
- [Trace Recipe Run Correlation Tutorial](tutorials/recipe-run-correlation.md) - Follow one run across stderr pointers, final JSON, child PID, and JSONL logs
- [Scoped Workflow Closure](concepts/scoped-workflow-closure.md) - Explicit persisted PR and process ownership model for default-workflow closure
- [How to Configure Scoped Workflow Closure](howto/configure-scoped-workflow-closure.md) - Pass repository, branch, base, PR, tracking item, run, and start-time scope through workflow closure
- [Tutorial: Scoped Workflow Closure](tutorials/scoped-workflow-closure.md) - Walk through scoped PR validation and stale process rejection
- [Scoped Workflow Closure Reference](reference/scoped-workflow-closure.md) - Helper arguments, JSON schemas, state fields, Rust validation API, and failure semantics

**Quick Start**:

```bash
# List available recipes
amplihack recipe list

# Execute a workflow recipe
amplihack recipe run default-workflow \
  -c task_description="Add user authentication" \
  -c repo_path=.

# Validate recipe YAML
amplihack recipe validate my-workflow.yaml

# Show recipe details
amplihack recipe show default-workflow
```

### Advanced Workflows

- [N-Version Programming](claude/workflow/N_VERSION_WORKFLOW.md) - Multiple solutions for critical code
- [Multi-Agent Debate](claude/workflow/DEBATE_WORKFLOW.md) - Structured decision-making
- [Cascade Workflow](claude/workflow/CASCADE_WORKFLOW.md) - Graceful degradation patterns

### GitHub Actions Workflows

- [Configure Issue Classifier](howto/configure-issue-classifier-workflow.md) - Permissions, timeout, label extension, lock file recompilation, and troubleshooting
- [Investigate Automated Action Failures](howto/investigate-automated-action-failures.md) - Evidence-first workflow for failing scheduled or automated GitHub Actions runs, root-cause classification, fixes, regression coverage, and PR closure
- [Automated Action Failure Investigation Reference](reference/automated-action-failure-investigation.md) - Inputs, configuration, evidence schema, classification values, PR contract, and closure semantics
- [Tutorial: Automated Action Failure Investigation](tutorials/automated-action-failure-investigation.md) - End-to-end example from user report to merge-ready PR

---

## 🤖 Agents & Tools

Specialized AI agents and tools for every development task.

### Core Agents

<!-- - [Agents Overview](claude/agents/README.md) - Complete agent catalog (see individual agent docs below) -->

- [Architect](claude/agents/amplihack/core/architect.md) - System design and specifications
- [Builder](claude/agents/amplihack/core/builder.md) - Code implementation from specs
- [Reviewer](claude/agents/amplihack/core/reviewer.md) - Quality assurance and compliance
- [Tester](claude/agents/amplihack/core/tester.md) - Test generation and validation

### Specialized Agents

- [API Designer](claude/agents/amplihack/core/api-designer.md) - Contract definitions
- [Security Agent](claude/agents/amplihack/specialized/security.md) - Vulnerability assessment
- [Database Agent](claude/agents/amplihack/specialized/database.md) - Schema and query optimization
- [Integration Agent](claude/agents/amplihack/specialized/integration.md) - External service connections
- [Cleanup Agent](claude/agents/amplihack/specialized/cleanup.md) - Code simplification

### Goal-Seeking Agents

**Autonomous agents that learn, remember, teach, and apply knowledge across four SDK backends.**

**📚 Tutorials (Learning-Oriented)**

- **[Goal-Seeking Agent Tutorial](tutorials/GOAL_SEEKING_AGENT_TUTORIAL.md)** - Interactive 10-lesson tutorial covering agent generation, SDK selection, multi-agent architecture, evaluations (L1-L12), and self-improvement loops

**📖 How-To Guides (Problem-Oriented)**

- [Goal Agent Generator Guide](GOAL_AGENT_GENERATOR_GUIDE.md) - Create custom goal-seeking agents with `amplihack new`
- [SDK Adapters Guide](SDK_ADAPTERS_GUIDE.md) - Choose and configure Copilot, Claude, Microsoft, or Mini SDK backends

**📋 Reference (Information-Oriented)**

- **[Quick Reference Card](reference/goal-seeking-agents-quick-reference.md)** - Fast lookup: commands, SDK selection, eval levels, common patterns
- [Eval System Architecture](EVAL_SYSTEM_ARCHITECTURE.md) - Progressive test suite (L1-L12), grading pipeline, domain agents, long-horizon memory eval, self-improvement runner
- [Goal Agent Generator Design](agent-bundle-generator-design.md) - Architecture and design patterns
- [Goal Agent Requirements](agent-bundle-generator-requirements.md) - Technical specifications
- [Implementation Summary](goal_agent_generator/IMPLEMENTATION_SUMMARY.md) - Current implementation status

**💡 Explanation (Understanding-Oriented)**

- **[Comprehensive Guide](GOAL_SEEKING_AGENTS.md)** - Complete system overview: capabilities, architecture, memory systems, evaluation framework, and self-improvement
- [Goal Agent Generator Presentation](GOAL_AGENT_GENERATOR_PRESENTATION.md) - High-level concept introduction

**Key Features**:

- **SDK-Agnostic**: Write once, run on Copilot, Claude, Microsoft Agent Framework, or lightweight mini-framework
- **7 Learning Tools**: learn, search, explain, verify, find gaps, store, summary
- **Progressive Eval (L1-L12)**: From simple recall to far transfer across domains
- **3-Run Median Eval**: `--runs 3` for stable benchmarks (reduces LLM stochasticity)
- **Multi-Vote Grading**: `--grader-votes 3` for noise reduction on ambiguous answers
- **Teaching Evaluation**: Multi-turn teacher-student knowledge transfer (Chi 1994, Vygotsky ZPD)
- **Self-Improvement Loop**: EVAL -> ANALYZE -> RESEARCH -> IMPROVE -> RE-EVAL -> DECIDE with automated error analysis
- **5 Domain Agents**: Code Review, Meeting Synthesizer, Data Analysis, Document Creator, Project Planning
- **Long-Horizon Memory Eval**: 1000-turn dialogue stress test
- **Multi-SDK Comparison**: 4-way eval comparison across Copilot, Claude, Microsoft Agent Framework, and mini SDK
- **Current Score**: 97.5% overall median (L1-L7, 3-run median, mini SDK)

### Memory-Enabled Agents ⭐ NEW

**Learning agents that improve through experience and persist knowledge across sessions.**

- [Feature Overview](features/memory-enabled-agents.md) - What are memory-enabled agents and when to use them
- [Getting Started Tutorial](tutorials/memory-enabled-agents-getting-started.md) - Create and run your first learning agent (30 minutes)
- [API Reference](reference/memory-enabled-agents-api.md) - Complete technical documentation for amplihack-memory-lib
- [Architecture Deep-Dive](concepts/memory-enabled-agents-architecture.md) - System design and technical details
- [How-To: Integrate Memory](howto/integrate-memory-into-agents.md) - Add memory to existing agents
- [How-To: Design Learning Metrics](howto/design-custom-learning-metrics.md) - Track domain-specific improvements
- [How-To: Validate Learning](howto/validate-agent-learning.md) - Test learning behavior with gadugi-agentic-test

**Key Features**:

- **Native memory API**: use the bundled memory commands and libraries from the installed `amplihack` CLI
- **Persistent Memory**: SQLite-based storage (no external database required)
- **Pattern Recognition**: Automatically recognize recurring situations after 3 occurrences
- **Learning Metrics**: Track runtime improvement, pattern recognition rate, confidence growth
- **Four Experience Types**: SUCCESS, FAILURE, PATTERN, INSIGHT
- **Validated Learning**: Test-driven validation ensures agents actually learn

**Demonstration Agents**:

1. **Documentation Analyzer** - Learns documentation quality patterns (MS Learn integration)
2. **Code Pattern Recognizer** - Identifies reusable code patterns and abstractions
3. **Bug Predictor** - Predicts likely bug locations based on learned characteristics
4. **Performance Optimizer** - Learns performance anti-patterns and optimization techniques

### Meta-Agentic Task Delegation ⭐ NEW

**Run AI agents in isolated subprocess environments with automatic validation and evidence collection.**

- [Meta-Delegation Overview](meta-delegation/README.md) - What is meta-delegation and when to use it
- [Tutorial](meta-delegation/tutorial.md) - Learn meta-delegation step-by-step (30 minutes)
- [How-To Guide](meta-delegation/howto.md) - Common tasks and recipes
- [API Reference](meta-delegation/reference.md) - Complete technical documentation
- [Concepts](meta-delegation/concepts.md) - Architecture and design principles
- [Troubleshooting](meta-delegation/troubleshooting.md) - Fix common issues

**Key Feature**: Delegate complex tasks to specialized personas (guide, QA engineer, architect, junior developer) running in isolated environments. The system monitors execution, collects evidence, validates success criteria, and provides detailed reports.

### Workflow Agents

- [Ambiguity Handler](claude/agents/amplihack/specialized/ambiguity.md) - Clarify unclear requirements
- [Optimizer](claude/agents/amplihack/core/optimizer.md) - Performance improvements
- [Pattern Recognition](claude/agents/amplihack/specialized/patterns.md) - Identify reusable solutions

### Claude Code Skills

Modular, on-demand capabilities that extend amplihack:

- [Skills Catalog](skills/SKILL_CATALOG.md) - Complete skills catalog
<!-- - [Documentation Writing](claude/skills/documentation-writing/SKILL.md) - Eight Rules compliance (Coming soon) -->
- [Mermaid Diagrams](claude/skills/mermaid-diagram-generator/SKILL.md) - Visual documentation
- [Test Gap Analyzer](claude/skills/test-gap-analyzer/SKILL.md) - Find untested code
- [Code Smell Detector](claude/skills/code-smell-detector/SKILL.md) - Identify anti-patterns

### Scenario Tools

Production-ready executable tools following the Progressive Maturity Model:

- [Scenario Tools Overview](claude/scenarios/README.md) - Progressive maturity model
- [Create Your Own Tools](CREATE_YOUR_OWN_TOOLS.md) - Build custom tools
- [Agent Bundle Generator](agent-bundle-generator-guide.md) - Package agents for distribution
- [GitHub Distribution](features/github-distribution.md) - Publish bundles to GitHub via `gh` CLI

#### Available Tools

- **[Platform Bridge](platform-bridge/README.md)** - Multi-platform support for GitHub and Azure DevOps
  - Automatic platform detection from git remotes
  - Unified API for both GitHub and Azure DevOps
  - Zero configuration required
  - Used by DEFAULT_WORKFLOW for cross-platform compatibility

---

## ⚡️ Commands & Operations

Execute complex tasks with simple slash commands.

### Command Reference

- [Command Selection Guide](commands/COMMAND_SELECTION_GUIDE.md) - Choose the right command for your task
- [Amplifier Command](reference/amplifier-command.md) - Launch Amplifier with the amplihack bundle and argv-only prompt delivery
- [resolve-bundle-asset Command](reference/resolve-bundle-asset-command.md) - Resolve native bundle assets and legacy parity aliases

### Operations

- [Operations Index](operations/README.md) - Release canaries, cleanup, workflow resilience, downstream validation, and roadmap docs
- [v0.10.6 Canary Evidence](operations/v0.10.6-canary-evidence.md) - Sanitized install/update/default-workflow evidence contract and current availability limitations
- [Hygiene Cleanup](operations/hygiene-cleanup.md) - Conservative opt-in cleanup for stale worktrees, detached Cargo targets, and session artifacts
- [Workflow Publish and Finalize Resilience](operations/workflow-resilience.md) - Idempotent no-diff, existing-PR, already-merged, and closed-after-merge terminal states
- [Prompt Delivery Downstream Validation](operations/prompt-delivery-downstream-validation.md) - Simard/RabbitHole-style prompt delivery validation scenarios
- [Operational Roadmap](operations/operational-roadmap.md) - Curated backlog for fleet rollout, Azure DevOps E2E, release contracts, and observability

### Core Commands

- `/ultrathink` - Main orchestration command (reads workflow, orchestrates agents)
- `/analyze` - Comprehensive code review for philosophy compliance
- `/improve` - Capture learnings and self-improvement
- `/fix` - Intelligent fix workflow for common error patterns

### Document-Driven Development Commands

- `/ddd:0-help` - Get help and understand DDD
- `/ddd:prime` - Prime context with DDD overview
- `/ddd:1-plan` - Phase 0: Planning & Alignment
- `/ddd:2-docs` - Phase 1: Documentation Retcon
- `/ddd:3-code-plan` - Phase 3: Implementation Planning
- `/ddd:4-code` - Phase 4: Code Implementation
- `/ddd:5-finish` - Phase 5: Testing & Phase 6: Cleanup
- `/ddd:status` - Check current phase and progress

### Advanced Commands

- `/amplihack:n-version <task>` - Generate N independent solutions for critical code
- `/amplihack:debate <question>` - Multi-agent structured debate for decisions
- `/amplihack:cascade <task>` - Fallback cascade for resilient operations
- `/amplihack:customize` - Manage user preferences and settings

### Auto Mode

- [Auto Mode Guide](AUTO_MODE.md) - Autonomous multi-turn execution
- [Auto Mode Safety](AUTOMODE_SAFETY.md) - Safety guardrails and best practices

---

## 🧠 Memory & Knowledge

Persistent memory systems and knowledge management.

### 5-Type Memory System ⭐ NEW

Psychological memory model with episodic, semantic, procedural, prospective, and working memory:

- [5-Type Memory Guide](memory/5-TYPE-MEMORY-GUIDE.md) - Complete user guide
- [Developer Reference](memory/5-TYPE-MEMORY-DEVELOPER.md) - Architecture and API
- [Quick Reference](memory/5-TYPE-MEMORY-QUICKREF.md) - One-page cheat sheet
- [Kùzu Memory Schema](memory/KUZU_MEMORY_SCHEMA.md) - Graph database design for 5 memory types
- [Kùzu Code Schema](memory/KUZU_CODE_SCHEMA.md) - Code graph schema for memory-code linking
- [Terminal Visualization](memory/MEMORY_TREE_VISUALIZATION.md) - View graph in terminal
- [Memory System Overview](memory/README.md) - Complete memory documentation

### Memory Systems

- [Agent Memory Integration](AGENT_MEMORY_INTEGRATION.md) - How agents share and persist knowledge
- [Agent Memory Quickstart](AGENT_MEMORY_QUICKSTART.md) - Get started with memory
- [Agent Type Memory Sharing](agent_type_memory_sharing_patterns.md) - Patterns for memory collaboration

### Kuzu Memory System

Embedded graph-based memory using Kuzu (NO Neo4j required):

- [Documentation Graph](doc_graph_quick_reference.md) - Navigate documentation connections
- [Code Context Injection](memory/CODE_CONTEXT_INJECTION.md) - Link code to memories

### Code Graph

Query your codebase structure via the Kuzu graph database:

- **[How to Use Blarify Code Graph](howto/blarify-code-graph.md)** - Enable, query, and configure

```bash
amplihack query-code stats
amplihack query-code search <name>
amplihack query-code functions --file <path>
```

**Current References**:

- [Documentation Knowledge Graph](documentation_knowledge_graph.md) - Documentation graph architecture and workflows
- [Blarify Code Graph Integration](blarify_integration.md) - Kuzu-backed code graph indexing and retrieval

### Memory Testing

- [Memory System Guide](memory/README.md) - Overview of the current Kuzu-backed memory stack
- [Testing Strategy](memory/TESTING_STRATEGY.md) - Validation approach for memory behavior
- [Effectiveness Test Design](memory/EFFECTIVENESS_TEST_DESIGN.md) - How we measure success

### External Knowledge

- [External Knowledge Integration](external_knowledge_integration.md) - Import external data sources

### Distributed Hive Mind ⭐ NEW

Multi-agent distributed memory system enabling agents to share knowledge across a gossip-based graph network.

- [Overview](distributed_hive_mind.md) - Architecture overview and design goals
- [Architecture](hive_mind/ARCHITECTURE.md) - Technical architecture: DHT shards, CRDT gossip, event bus
- [Design](hive_mind/DESIGN.md) - Design decisions, data model, and trade-offs
- [Getting Started](hive_mind/GETTING_STARTED.md) - Deploy a local hive mind in minutes
- [Tutorial](hive_mind/TUTORIAL.md) - Step-by-step guide to building distributed agents
- [Evaluation](hive_mind/EVAL.md) - Benchmarks, eval scenarios, and performance results
- [Presentation](hive_mind_presentation.md) - High-level slides and demo walkthrough
- [Prompt-to-Hive Tutorial](tutorial_prompt_to_distributed_hive.md) - End-to-end walkthrough from prompt to deployed hive
- [Hive Mind Getting Started](tutorials/hive-mind-getting-started.md) - Diataxis tutorial for the Rust hive runtime
- [Hive Mind Tutorial](tutorials/hive-mind-tutorial.md) - Build and operate hive-mind workflows step by step
- [Hive Mind Design](concepts/hive-mind-design.md) - Design concepts for the Rust hive implementation
- [Hive Mind Evaluation](concepts/hive-mind-eval.md) - Evaluation model and criteria for hive-mind behavior

**Key Features**:

- **DHT-based sharding**: Consistent-hash ring distributes facts across agent shards
- **CRDT gossip**: Bloom-filter gossip protocol for eventual consistency without conflicts
- **Azure Service Bus transport**: Cross-process event bus for production deployments
- **NetworkGraphStore**: Pluggable transport layer wrapping any local GraphStore
- **Kuzu-backed shards**: Each shard uses Kuzu embedded graph for POSIX-safe persistent storage

---

### Blarify Code Indexing

Complete code indexing and analysis with multi-language support:

- **[How to Use Blarify Code Graph](howto/blarify-code-graph.md)** - Enable, query, and configure code graph indexing
- [Blarify Integration](blarify_integration.md) - Technical integration details
- [Blarify Quickstart](blarify_quickstart.md) - Get started in 5 minutes

---

## 🔧 Features & Integrations

Specific features and third-party integrations.

### Native Binary Trace Logging ⭐ NEW

Optional request/response logging using Anthropic's native Claude binary:

- **[Native Binary Trace Logging Overview](NATIVE_BINARY_TRACE_LOGGING.md)** - Complete feature documentation hub
- [Trace Logging Feature Guide](features/trace-logging.md) - What it is and when to use it
- [How-To: Trace Logging](howto/trace-logging.md) - Practical recipes
- [API Reference: Trace Logging](reference/trace-logging-api.md) - Technical details
- [Troubleshooting: Trace Logging](troubleshooting/trace-logging.md) - Fix common issues

**Key Features**: Zero overhead when disabled (<0.1ms), automatic security sanitization, session-scoped JSONL logs, no NPM dependencies.

### Power Steering

Intelligent guidance system that prevents common mistakes:

- [Power Steering Overview](features/power-steering/README.md) - What is Power Steering
- [Configuration Guide](features/power-steering/configuration.md) - Complete configuration reference
- [Customization Guide](features/power-steering/customization-guide.md) - Customize considerations
- [Troubleshooting](features/power-steering/troubleshooting.md) - Fix common issues
- [Migration Guide v0.9.1](features/power-steering/migration-v0.9.1.md) - Upgrade guide
- [Changelog v0.9.1](features/power-steering/changelog-v0.9.1.md) - Infinite loop fix release notes

**Compaction Handling** ⭐ NEW

Robust handling of conversation compaction in long sessions:

- [Compaction Overview](power_steering_compaction_overview.md) - What is compaction and how power-steering handles it
- [Compaction API Reference](power_steering_compaction_api.md) - Developer documentation for CompactionValidator and CompactionContext
- [How to Customize Power Steering](../amplifier-bundle/tools/amplihack/HOW_TO_CUSTOMIZE_POWER_STEERING.md#compaction-handling) - Configuration and troubleshooting

### Other Features

- [Smart Memory Management](features/smart-memory-management.md) - Automatic Node.js memory optimization for Claude Code
- [Claude.md Preservation](features/claude-md-preservation.md) - Preserve custom instructions
  <!-- Neo4j removed - now using Kuzu embedded database (no session cleanup needed) -->
  <!-- - Shutdown Detection - Graceful exit handling (see stop-hook-exit-hang in Troubleshooting) -->

### Third-Party Integrations

- [MCP Evaluation](mcp_evaluation/README.md) - Model Context Protocol evaluation

---

## ⚙️ Configuration & Deployment

Advanced configuration, deployment patterns, and environment management.

### Configuration

- [Profile Management](PROFILE_MANAGEMENT.md) - Multiple environment configurations
- [Hook Configuration](HOOK_CONFIGURATION_GUIDE.md) - Customize framework behavior
- [Shell Command Hook](SHELL_COMMAND_HOOK.md) - Custom shell integrations

### Deployment

- [First-Time Install](howto/first-install.md) - Install from source, npm bootstrap, or release binaries
- [Install Manifest](reference/install-manifest.md) - Understand installed files and uninstall state
- [Azure Integration](AZURE_INTEGRATION.md) - Deploy to Azure cloud

### Build System

- [Install Manifest](reference/install-manifest.md) - Installed-file manifest and uninstall state

### Remote Sessions

- [Remote Sessions Overview](remote-sessions/index.md) - Execute on remote machines
- [Remote Sessions User Guide](remote-sessions/index.md) - Set up and operate remote sessions
- [Remote Sessions Tutorial](remote-sessions/TUTORIAL.md) - Walk through an end-to-end remote workflow

---

## 🧪 Testing & Quality

Testing strategies, quality assurance, and validation patterns.

### Testing

- [Benchmarking](BENCHMARKING.md) - Performance measurement and comparison
- [Test Gap Analyzer](claude/skills/test-gap-analyzer/SKILL.md) - Find untested code
- [CS Validator](cs-validator/README.md) - Code style validation
- [Testing Plan](testing/TEST_PLAN.md) - Testing strategy and execution checklist

### Code Review

- [Code Review Guide](CODE_REVIEW.md) - Review process and standards
- [Memory Code Review](memory/CODE_REVIEW_PR_1077.md) - Example: Memory system review

---

## 🔒 Security

Security guidelines, context preservation, and best practices.

### Security Documentation

- [Security Recommendations](SECURITY_RECOMMENDATIONS.md) - Essential security practices
- [Security Context Preservation](SECURITY_CONTEXT_PRESERVATION.md) - Maintain security through sessions
- [Security Guides](security/README.md) - Comprehensive security documentation

### Safe Operations

- [Auto Mode Safety](AUTOMODE_SAFETY.md) - Autonomous operation guardrails

---

## 🛠️ Troubleshooting & Discoveries

Fix common issues and learn from past solutions.

### Troubleshooting

- [Discoveries](https://rysweet.github.io/amplihack-rs/discoveries/) - Known issues and solutions (CHECK HERE FIRST!)
- [Troubleshooting Guides](troubleshooting/README.md) - Common problems and fixes
- [Memory Consent Issues](troubleshooting/memory-consent-issues.md) - Fix prompt, timeout, and detection problems
- [Memory-Enabled Agents Issues](troubleshooting/memory-enabled-agents.md) - Fix memory persistence, pattern recognition, and learning problems
- [Platform Bridge Troubleshooting](troubleshooting/platform-bridge.md) - Fix platform detection and CLI issues
- [Stop Hook Exit Hang](troubleshooting/stop-hook-exit-hang.md) - Fix 10-13s hang on exit (resolved v0.9.1)

### Documentation Guides

- [Documentation Guidelines](DOCUMENTATION_GUIDELINES.md) - Writing effective docs
- [Documentation Knowledge Graph](documentation_knowledge_graph.md) - Graph-based doc navigation and indexing
- [How to Generate GitHub Pages](howto/github-pages-generation.md) - Publish your docs

### How-To Guides

- [Exception Handling](howto/exception-handling.md) - Implement proper error handling in amplihack code
- [Configure Memory Consent](howto/configure-memory-consent.md) - Customize prompt behavior, timeouts, and CI/CD integration
- [Configure Power-Steering Merge Preferences](howto/power-steering-merge-preferences.md) - Set up merge approval workflow
- [Platform Bridge Quick Start](tutorials/platform-bridge-quickstart.md) - Learn the basics in 10 minutes
- [Platform Bridge Workflows](howto/platform-bridge-workflows.md) - Common workflows for GitHub and Azure DevOps
- [Crusty Old Engineer](howto/use-crusty-old-engineer.md) - Skeptical engineering advisor for architecture and tooling decisions

---

## 🔬 Research & Advanced Topics

Cutting-edge research, experimental features, and deep technical dives.

### Research Projects

- [Blarify Code Graph Integration](blarify_integration.md) - Code graph indexing with Kuzu-backed memory
- [Documentation Knowledge Graph](documentation_knowledge_graph.md) - Graph-based documentation retrieval
- [External Knowledge Integration](external_knowledge_integration.md) - Import external data sources into memory workflows

### Advanced Topics

- [Agent Type Memory Sharing Patterns](agent_type_memory_sharing_patterns.md) - Advanced memory patterns
- [Documentation Knowledge Graph](documentation_knowledge_graph.md) - Graph-based doc navigation
- [Workspace Pattern](WORKSPACE_PATTERN.md) - Advanced workspace organization

---

## 📚 Reference & Resources

Quick references, guides, and additional resources.

### Quick References

- [Exception Handling Reference](reference/exception-handling.md) - Complete exception hierarchy and patterns
- [Command Selection Guide](commands/COMMAND_SELECTION_GUIDE.md) - Choose the right command
- [Platform Bridge API Reference](reference/platform-bridge-api.md) - Complete API documentation
- [Power Steering File Locking](reference/power-steering-file-locking.md) - Prevents counter race conditions
- [UserPromptSubmit Hook API Reference](reference/user-prompt-submit-hook-api.md) - Framework injection API
- [Doc Graph Quick Reference](doc_graph_quick_reference.md) - Navigate documentation

### Developing amplihack

- [Developing amplihack](DEVELOPING_AMPLIHACK.md) - Contribute to the framework
- [Create Your Own Tools](CREATE_YOUR_OWN_TOOLS.md) - Build custom tools
- [Workflow to Skills Migration](WORKFLOW_TO_SKILLS_MIGRATION.md) - Migration guide

### Contributing

- [File Organization](contributing/file-organization.md) - Where different file types belong in the repository
- [Discoveries](discoveries.md) - Patterns and insights discovered during development

### Investigations

- [#434 gherkin-expert disposition](investigations/434-gherkin-v2-experiment-disposition.md) - Investigation confirming the gherkin-expert skill already ships in amplihack

### GitHub & Community

- [GitHub Repository](https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding) - Source code
- [Issue Tracker](https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding/issues) - Report bugs or request features
- [GitHub Pages](https://rysweet.github.io/amplihack-rs/) - Online documentation

---

## Philosophy in Action

amplihack follows three core principles:

1. **Ruthless Simplicity**: Start with the simplest solution that works. Add complexity only when justified.

2. **Modular Design**: Build self-contained modules (bricks) with clear public contracts (studs) that others can connect to.

3. **Zero-BS Implementation**: No stubs, no placeholders, no dead code. Every function must work or not exist.

---

## Example Workflow

```bash
# Start with a feature request
/ultrathink "Add user authentication to the API"

# UltraThink will:
# 1. Read the default workflow
# 2. Orchestrate multiple agents (architect, security, api-designer, database, builder, tester)
# 3. Follow all workflow steps systematically
# 4. Ensure quality and philosophy compliance
# 5. Generate tests and documentation
```

---

## Use Cases

amplihack excels at:

- **Feature Development**: Orchestrate multiple agents to design, implement, test, and document new features
- **Code Review**: Comprehensive analysis for philosophy compliance and best practices
- **Refactoring**: Systematic cleanup and improvement of existing code
- **Investigation**: Deep understanding of complex codebases and architectures
- **Integration**: Connect external services with proper error handling and testing
- **Security**: Vulnerability assessment and secure implementation patterns

---

## Need Help?

- **Start here**: [Prerequisites](PREREQUISITES.md) → [Interactive Installation](INTERACTIVE_INSTALLATION.md) → [Quick Start](../README.md)
- **Common issues**: Check [Discoveries](discoveries.md) first
- **Questions**: See [DDD FAQ](document_driven_development/reference/faq.md)
- **Report issues**: [GitHub Issues](https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding/issues)

---

**Ready to get started?** Head to [Prerequisites](PREREQUISITES.md) to set up amplihack in your development environment.
