# Skip Pre-Agent Validation on Large Codebases

How to prevent `smart-orchestrator` and `default-workflow` from running pre-agent project validation before the agent starts working — avoiding unnecessary overhead on large codebases.

## Before you start

- `amplihack` is installed (`amplihack --version`)
- You have a recipe to run (`smart-orchestrator.yaml` or `default-workflow.yaml`)
- Your repository has a slow test suite or build step (5+ minutes)

---

## The short version

Pre-agent validation is **off by default**. If you have not changed `skip_pre_agent_validation` in your recipe files, validation is already skipped. No action needed.

---

## Why this exists

The default workflow's step-01 (prepare workspace) contains a guard block where project-level validation commands can be added before the agent step begins. On medium-to-large codebases with slow test suites, this can delay productive work by minutes per round — and the same validation runs again at the end of each round, making the pre-agent pass redundant. The guard block initially contains echo stubs; project-specific commands (test suites, build steps) are added per-repository.

The `skip_pre_agent_validation` context variable defaults to `"true"` so the agent starts immediately. The agent's own post-work validation catches any issues.

---

## Opt in to pre-agent validation

If your workflow requires a known-good baseline before the agent starts (for example, a CI-gated codebase where you want to confirm the tree is clean before any modifications):

```sh
amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
  -c task_description="refactor auth module" \
  -c repo_path="." \
  -c skip_pre_agent_validation="false"
```

The `-c skip_pre_agent_validation="false"` flag tells step-01 to execute the project validation guard block before the agent step.

---

## Verify the setting

Dry-run the recipe and check the resolved context:

```sh
amplihack recipe show amplifier-bundle/recipes/smart-orchestrator.yaml
```

Look for `skip_pre_agent_validation` in the context block. The default value is `"true"` (validation skipped).

To confirm the variable reaches workflow-prep at runtime, run with `--verbose`:

```sh
amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
  -c task_description="test validation flow" \
  -c repo_path="." \
  -c skip_pre_agent_validation="false" \
  --verbose
```

In the verbose output, step-01 prints `Running pre-agent project validation...` when validation is active. When skipped (the default), no validation message appears.

---

## Permanent override in recipe YAML

To always run pre-agent validation for a specific project, edit the recipe's context block:

```yaml
# In amplifier-bundle/recipes/smart-orchestrator.yaml
context:
  skip_pre_agent_validation: "false"   # was "true"
```

Apply the same change in `default-workflow.yaml` if you run that recipe directly.

**Trade-off:** This runs any configured validation commands before every agent round. On codebases with slow test suites or build steps, expect meaningful overhead per round.

---

## Related

- [skip_pre_agent_validation Reference](../reference/skip-pre-agent-validation.md) — Type, propagation chain, and bash consumption details
- [Run a Recipe End-to-End](./run-a-recipe.md) — General recipe execution guide
- [Troubleshoot Recipe Execution](./troubleshoot-recipe-execution.md) — Diagnosing slow or stuck recipes
