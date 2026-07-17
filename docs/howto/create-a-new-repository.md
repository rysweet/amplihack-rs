# Create a New Repository from a Workflow Task

How to run `smart-orchestrator` or `default-workflow` against an empty (non-git)
directory so the workflow scaffolds a brand-new repository instead of failing at
the first step.

## Before you start

- `amplihack` is installed (`amplihack --version`)
- You have a recipe to run (`smart-orchestrator.yaml` or `default-workflow.yaml`)
- Your task goal is to **create** a new repository — the target directory does
  not yet contain a `.git` directory

---

## The short version

Auto-init is **on by default**. Point the recipe at any directory — empty or
not — and step-01 initializes a git repository on the `main` branch when one
does not already exist. No action needed:

```sh
amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
  -c task_description="scaffold a new Rust CLI project called widgetron" \
  -c repo_path="./widgetron"
```

When step-01 runs against the non-git directory it prints:

```
[init] no git repo found — initialized a new one for repo-creation task
```

and continues through the rest of the workflow.

---

## Why this exists

Previously, `workflow-prep` step-01 (`step-01-prepare-workspace`) hard-failed
with `exit 1` whenever `repo_path` was not inside a git work tree. That guard
protected real development tasks from running against a stray directory — but it
also made it impossible to use the workflow for its second major use case:
creating a repository that does not exist yet (issue #900).

The `auto_init_repo` context variable resolves the conflict. It defaults to
`"true"`, so:

- **Repo-creation tasks** (empty/non-git dir) → step-01 runs `git init -b main`
  and proceeds.
- **Normal development tasks** (existing checkout) → step-01 behavior is
  unchanged: full status, fetch, and branch diagnostics run exactly as before.

Auto-init never touches an existing `.git` directory. It only fires when there
is no repository to begin with.

---

## Verify the initialized repository

After the workflow runs (or after step-01 alone), confirm the repo exists on the
`main` branch:

```sh
cd ./widgetron
git rev-parse --is-inside-work-tree   # -> true
git branch --show-current             # -> main   (unborn until first commit)
git rev-parse --abbrev-ref HEAD       # -> main
```

To watch the auto-init happen at runtime, run with `--verbose` and look for the
`[init]` line in step-01's output:

```sh
amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
  -c task_description="scaffold a new project" \
  -c repo_path="./widgetron" \
  --verbose
```

---

## Disable auto-init (fail-closed)

In locked-down or CI contexts that must **never** create a repository
implicitly, set `auto_init_repo="false"` to restore the pre-#900 behavior.
step-01 then hard-fails on a non-git directory:

```sh
amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
  -c task_description="refactor auth module" \
  -c repo_path="." \
  -c auto_init_repo="false"
```

With the flag disabled, running against a non-git directory prints:

```
ERROR: step-01-prepare-workspace requires a git repo at <path>; either `git init` or rerun from a checkout (set auto_init_repo=true to auto-initialize)
```

and exits `1`.

---

## Permanent override in recipe YAML

To change the default for a specific project, edit the recipe's context block:

```yaml
# In amplifier-bundle/recipes/smart-orchestrator.yaml
context:
  auto_init_repo: "false"   # was "true" — never auto-create repos for this project
```

Apply the same change in `default-workflow.yaml` if you run that recipe
directly.

**Trade-off:** With `auto_init_repo="false"`, any task pointed at a directory
without a `.git` will fail at step-01. Only set this when implicit repo creation
is genuinely undesirable.

---

## Older git versions

`git init -b main` requires git **2.28+**. step-01 falls back automatically on
older git (`git init` + `git checkout -b main`, then a `git symbolic-ref HEAD`
last resort), so the initialized repo always lands on `main` regardless of host
git version. No configuration is required.

---

## Related

- [auto_init_repo Reference](../reference/auto-init-repo.md) — Type, default,
  propagation chain, fallback chain, and bash consumption details
- [Skip Pre-Agent Validation on Large Codebases](./skip-pre-agent-validation.md)
  — Sibling step-01 context variable
- [Run a Recipe End-to-End](./run-a-recipe.md) — General recipe execution guide
- [Troubleshoot Recipe Execution](./troubleshoot-recipe-execution.md) —
  Diagnosing recipe failures
