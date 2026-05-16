# How to Run a Quality Audit

Run the `quality-audit-cycle` recipe to scan a codebase for quality issues with
escalating-depth SEEK/VALIDATE/FIX/RECURSE cycles.

## Prerequisites

- amplihack installed (`AMPLIHACK_HOME` set)
- `amplihack` CLI binary on PATH
- Target repository checked out locally

## Basic Invocation

```bash
amplihack recipe run quality-audit-cycle \
  -c task_description="Run quality audit on the payments module" \
  -c repo_path="." \
  -c target_path="src/payments" \
  --verbose
```

> **Note:** Use `amplihack recipe run` — not `amplihack recipe execute`. The
> CLI is the canonical invocation path, matching how `dev-orchestrator`
> and all other recipes are launched.

## Setting `repo_path`

The `repo_path` variable tells agent steps where the repository root is. Set it
so that `target_path` resolves relative to the repo:

```bash
amplihack recipe run quality-audit-cycle \
  -c task_description="Audit the crates directory" \
  -c repo_path="/home/user/src/my-project" \
  -c target_path="crates/" \
  --verbose
```

When `repo_path` is set, each agent step's `working_dir` is set to that path,
giving agents file-system access to the target directory.

**Rules:**

| `repo_path`             | `target_path`  | Agent sees                             |
| ----------------------- | -------------- | -------------------------------------- |
| `.` (default)           | `src/payments` | `./src/payments` from CWD              |
| `/home/user/src/myproj` | `crates/`      | `crates/` relative to `/home/…/myproj` |
| (omitted)               | absolute path  | Works, but agents may lack CWD context |

## Targeting a Subdirectory

Set `target_path` to audit a specific part of the codebase:

```bash
amplihack recipe run quality-audit-cycle \
  -c target_path="src/amplihack/fleet" \
  -c repo_path="." \
  -c min_cycles="2" \
  -c max_cycles="4" \
  --verbose
```

## Filtering by Category

Limit the audit to specific issue categories:

```bash
amplihack recipe run quality-audit-cycle \
  -c target_path="src/amplihack" \
  -c repo_path="." \
  -c categories="security,reliability,error_swallowing" \
  --verbose
```

Available categories: `security`, `reliability`, `dead_code`, `silent_fallbacks`,
`error_swallowing`, `result_dropping`, `shell_anti_patterns`, `silent_truncation`,
`async_anti_patterns`, `config_divergence`, `validation_gaps`, `health_observability`,
`retry_anti_patterns`, `structural`, `hardcoded_limits`, `test_gaps`, `doc_gaps`,
`documentation`.

## Adjusting Cycle Limits

```bash
amplihack recipe run quality-audit-cycle \
  -c target_path="src/amplihack" \
  -c repo_path="." \
  -c min_cycles="3" \
  -c max_cycles="6" \
  -c severity_threshold="high" \
  --verbose
```

## Troubleshooting

### "target path does not exist"

The agent cannot find `target_path`. Likely causes:

1. **Missing `repo_path`** — set `repo_path` to the repo root so
   agents resolve relative paths correctly.
2. **Relative path with wrong CWD** — ensure your shell's CWD is the repo root
   before running `amplihack recipe run`, or use an absolute `target_path`.

### Bash step errors like `json: command not found`

Template variables (`{{validated_findings}}`) are being interpreted as bash
commands instead of being interpolated. This is a heredoc safety issue —
see the [recipe reference](../reference/recipe-quick-reference.md)
for details on the fix.

### Recipe completes but agents produce empty results

This is a **hollow success**. Check:

1. `repo_path` points to the actual repo root
2. `target_path` contains files the agents can read
3. The agent binary has access to file-reading tools

## See Also

- [Quality Audit Recipe Reference](../reference/recipe-quick-reference.md) — full
  context variable table and step-by-step reference
- [SKILL.md](#) — skill activation
  triggers and detection categories
