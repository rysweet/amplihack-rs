# Workflow to Skills Migration Guide

**Status**: canonical skill/recipe architecture

## Architecture Change

**Before**: Workflows | Commands | Agents | Skills (4 mechanisms)
**After**: Skills backed by recipes | Commands | Agents (3 mechanisms)

Workflows are implemented as skills and executable recipes. The canonical default
development workflow is the `default-workflow` skill/recipe:

- **Skill documentation**: `amplifier-bundle/skills/default-workflow/SKILL.md`
- **Executable recipe**: `amplifier-bundle/recipes/default-workflow.yaml`
- **Normal entry point**: `dev-orchestrator`, which routes through
  `amplihack recipe run smart-orchestrator`
- **Direct compatibility entry point**:
  `amplihack recipe run default-workflow -c task_description="..." -c repo_path=.`

The default workflow exposes one direct executable recipe interface through the
Rust CLI recipe runner.

Generated preferences and bundled context describe the selected workflow as
`default-workflow` skill/recipe. They do not present
`~/.amplihack/.claude/workflow/DEFAULT_WORKFLOW.md` or
`~/.amplihack/.claude/workflows/DEFAULT_WORKFLOW.md` as canonical locations.

## User Preference Rendering

Fresh installs and generated session context render workflow configuration like
this:

```markdown
## Workflow Configuration

**Selected**: `default-workflow` skill/recipe
**Consensus Depth**: balanced

Use the `consensus-workflow` skill/recipe for: ambiguous requirements,
architectural changes, critical/security code, public APIs.
```

Agents use that preference text as routing guidance. Development, investigation,
and hybrid tasks enter `dev-orchestrator`; `dev-orchestrator` invokes
`smart-orchestrator`; `smart-orchestrator` selects `default-workflow`,
`investigation-workflow`, or another recipe as appropriate.

## Deprecated Files

Legacy markdown workflow files are compatibility references only. They are not
authoritative prompt instructions and are not the source of generated preference
guidance.

| Deprecated legacy file     | Canonical replacement          |
| -------------------------- | ------------------------------ |
| `DEFAULT_WORKFLOW.md`      | `default-workflow` skill/recipe |
| `INVESTIGATION_WORKFLOW.md` | `investigation-workflow` skill/recipe |
| `CASCADE_WORKFLOW.md`      | `cascade-workflow` skill/recipe |
| `CONSENSUS_WORKFLOW.md`    | `consensus-workflow` skill/recipe |
| `DEBATE_WORKFLOW.md`       | `debate-workflow` skill/recipe |
| `N_VERSION_WORKFLOW.md`    | `n-version-workflow` skill/recipe |

Legacy path resolution may still accept historical locations such as
`~/.amplihack/.claude/workflow/DEFAULT_WORKFLOW.md` for migration and old
installations. That fallback is explicitly deprecated compatibility behavior.
It must not be surfaced as the selected/default workflow in user-facing docs,
fresh setup output, generated preferences, session-start context, or agent
instructions.

## Usage

For normal development work, route through the orchestrator:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Fix stale workflow references" \
  -c repo_path=.
```

For direct standalone execution of the default workflow:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix stale workflow references" \
  -c repo_path=.
```

For agent runtimes that support skills, invoke `Skill(skill="dev-orchestrator")`
for DEV, INVESTIGATE, and HYBRID tasks. Directly invoking
`Skill(skill="default-workflow")` is reserved for explicit requests or
orchestrator-unavailable compatibility.

## Behavior Specification

The finished migration has these externally visible behaviors:

1. Generated user preferences select `` `default-workflow` skill/recipe ``.
2. Session-start context and bundled agent guidance route development work
   through `dev-orchestrator` and `smart-orchestrator`.
3. Standalone default workflow usage is documented as
   `amplihack recipe run default-workflow -c task_description="..." -c repo_path=.`
4. Legacy markdown workflow paths are described only as deprecated migration or
   backward-compatibility references.
5. Documentation does not instruct users or agents to read
   `DEFAULT_WORKFLOW.md` and manually execute the steps.

## Configuration

The selected workflow setting names the canonical skill/recipe, not a filesystem
path:

```yaml
selected_workflow: default-workflow
consensus_depth: balanced
```

Installers, generators, fixtures, and tests that render preference/context text
assert the canonical wording:

```markdown
**Selected**: `default-workflow` skill/recipe
```

They also assert that stale canonical wording is absent:

```text
~/.amplihack/.claude/workflow/DEFAULT_WORKFLOW.md
~/.amplihack/.claude/workflows/DEFAULT_WORKFLOW.md
@.claude/workflow/DEFAULT_WORKFLOW.md
```

Remaining references to `DEFAULT_WORKFLOW.md` are acceptable only in deprecated
file tables, compatibility notes, migration explanations, or explicit warnings
not to use the file as canonical guidance.

## Maintenance Audit

When changing workflow guidance, audit for stale canonical references:

```bash
rg 'DEFAULT_WORKFLOW\.md|\.claude/workflow/DEFAULT_WORKFLOW|@~/.amplihack/.claude/workflows/DEFAULT_WORKFLOW|run_recipe_by_name|from amplihack\.recipes|python3 -c' \
  docs amplifier-bundle
```

Classify each match before editing. Keep historical or compatibility references
when they are clearly labeled as deprecated; update matches that present legacy
markdown files, Python recipe imports, or `run_recipe_by_name` as the current
execution path.

`python3 -c` matches are stale only when they import or call workflow recipes;
unrelated Python verification snippets can remain.

## Related

- `amplifier-bundle/CLAUDE.md`: orchestrator and recipe entry-point guidance
- `amplifier-bundle/skills/default-workflow/SKILL.md`: canonical user-facing
  workflow documentation
- `amplifier-bundle/recipes/default-workflow.yaml`: canonical executable
  workflow definition
