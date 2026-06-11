# Legacy Markdown Workflow Files

This directory is a deprecated compatibility reference for older installations
that still contain markdown workflow files. It is not the canonical workflow
configuration surface.

Canonical workflows are skills and executable recipes:

| Legacy markdown file | Canonical replacement |
| --- | --- |
| `DEFAULT_WORKFLOW.md` | `default-workflow` skill/recipe |
| `INVESTIGATION_WORKFLOW.md` | `investigation-workflow` skill/recipe |
| `CONSENSUS_WORKFLOW.md` | `consensus-workflow` skill/recipe |
| `DEBATE_WORKFLOW.md` | `debate-workflow` skill/recipe |
| `CASCADE_WORKFLOW.md` | `cascade-workflow` skill/recipe |
| `N_VERSION_WORKFLOW.md` | `n-version-workflow` skill/recipe |

## Usage

For normal development work, route through `dev-orchestrator` and
`smart-orchestrator`:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Implement the change" \
  -c repo_path=.
```

For standalone default workflow execution, run the recipe directly:

```bash
amplihack recipe run default-workflow \
  -c task_description="Implement the change" \
  -c repo_path=.
```

Do not configure generated user preferences, session context, or agent prompts to
select `DEFAULT_WORKFLOW.md`. Generated guidance must use:

```markdown
**Selected**: `default-workflow` skill/recipe
```

## Compatibility

Legacy markdown workflow paths may still be recognized by migration and
backward-compatibility code. That support exists so old installations can be
upgraded safely; it does not make this directory authoritative.

When updating documentation, prefer links to:

- `../skills/default-workflow/SKILL.md`
- `../../../amplifier-bundle/recipes/default-workflow.yaml`
- `../../WORKFLOW_TO_SKILLS_MIGRATION.md`
